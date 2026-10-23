// Focused behavioral tests for the transport-independent application actions:
// strict route fields, version guards around awaited asset loads, one-commit
// patch semantics, idempotent submission and the deferred-request guard.

import { describe, expect, it } from "bun:test";
import type { Draft, ProviderId, WorkingImage } from "../lib/types";
import type { ImportedImage } from "../lib/api";
import { defaultsFor } from "../models/catalog";
import { McpError } from "./workspaceActions";
import {
  createApplicationActions,
  type ActionContext,
  type ApplicationHost,
  type McpSubmissionRecord,
} from "./applicationActions";

const uuid = () => crypto.randomUUID();

const image = (over: Partial<WorkingImage> = {}): WorkingImage => ({
  uid: uuid(),
  dataUrl: "data:image/png;base64,AAAA",
  width: 256,
  height: 256,
  name: "ref",
  ...over,
});

const draft = (over: Partial<Draft> = {}): Draft => ({
  schema: 4,
  intent: "create",
  family: "gpt",
  modelId: "gpt-image-2.5-flare",
  provider: "runware" as ProviderId,
  prompt: "a cat",
  params: { ...defaultsFor("gpt-image-2.5-flare", "runware") },
  refs: [],
  boxes: [],
  baseId: null,
  canvas: { w: 1024, h: 1024 },
  compressEnabled: false,
  maxInputEdge: 2048,
  mask: null,
  ...over,
});

interface FakeHost {
  host: ApplicationHost;
  commits: Draft[];
  submits: { count: number; mcp?: unknown }[];
  persists: Draft[];
  bump(): void;
  setVersion(v: string): void;
  ledger: Map<string, McpSubmissionRecord>;
}

const makeHost = (initial: Draft, version = "inst:0"): FakeHost => {
  let current = initial;
  let ver = version;
  const commits: Draft[] = [];
  const submits: { count: number; mcp?: unknown }[] = [];
  const persists: Draft[] = [];
  const ledger = new Map<string, McpSubmissionRecord>();
  const host: ApplicationHost = {
    ready: () => true,
    getVersion: () => ver,
    isGestureActive: () => false,
    canPersist: () => true,
    isBusy: () => false,
    submitBlocked: () => false,
    readCurrent: () => current,
    readIntent: () => current,
    commit: (next) => {
      current = next;
      commits.push(next);
      ver = `inst:${Number(ver.split(":")[1]) + 1}`;
    },
    persist: async (snapshot) => {
      persists.push(snapshot);
    },
    submit: async (_snapshot, count, mcp) => {
      submits.push({ count, mcp });
      const id = mcp?.idempotencyKey ?? uuid();
      if (mcp)
        ledger.set(mcp.idempotencyKey, {
          idempotencyKey: mcp.idempotencyKey,
          workspaceVersion: mcp.workspaceVersion,
          taskId: id,
        });
      return id;
    },
    stopRemaining: () => ({ stopped: false }),
    mcpSubmissionGet: async (key) => ledger.get(key) ?? null,
    readAsset: async (assetId): Promise<ImportedImage> => ({
      assetId,
      dataUrl: "data:image/png;base64,AAAA",
      width: 256,
      height: 256,
      name: "asset",
    }),
    maskFromRects: (image) => ({
      uid: uuid(),
      dataUrl: "data:image/png;base64,AAAA",
      width: image.width,
      height: image.height,
      name: "mask",
    }),
    makePreview: async () => "data:image/jpeg;base64,AAAA",
    providerStatus: () => ({ runware: true }),
  };
  return {
    host,
    commits,
    submits,
    persists,
    ledger,
    bump: () => {
      ver = `inst:${Number(ver.split(":")[1]) + 1}`;
    },
    setVersion: (v) => {
      ver = v;
    },
  };
};

const code = (e: unknown) => (e instanceof McpError ? e.code : String(e));

it("rejects invalid patch input without touching the draft", async () => {
  const cases: [string, Record<string, unknown>, string][] = [
    ["unknown route field", { params: { bogus: 1 } }, "PATCH_UNSUPPORTED"],
    ["wrong primitive type", { params: { quality: 5 } }, "PARAM_INVALID"],
    ["unsupported resetParams name", { resetParams: ["toString"] }, "PARAM_UNSUPPORTED"],
  ];
  for (const [name, patch, expected] of cases) {
    const fake = makeHost(draft());
    const actions = createApplicationActions(() => fake.host);
    const before = fake.host.readCurrent();
    const err = await actions
      .dispatch("workspace_patch", { expectedVersion: "inst:0", patch })
      .catch((e) => e);
    expect(code(err), name).toBe(expected);
    expect(fake.commits, name).toHaveLength(0);
    expect(fake.host.readCurrent(), name).toBe(before);
  }
});

describe("workspace_patch guards and commit", () => {
  it("rejects when the host version changes during an asset await", async () => {
    const fake = makeHost(draft({ intent: "edit", refs: [image()] }));
    const baseRead = fake.host.readAsset;
    fake.host.readAsset = async (id) => {
      const loaded = await baseRead(id);
      fake.bump(); // a user edit landed while the asset was loading
      return loaded;
    };
    const actions = createApplicationActions(() => fake.host);
    const err = await actions
      .dispatch("workspace_patch", {
        expectedVersion: "inst:0",
        patch: { refs: [{ assetId: uuid() }] },
      })
      .catch((e) => e);
    expect(code(err)).toBe("VERSION_CONFLICT");
    expect(fake.commits).toHaveLength(0);
  });

  it("rejects expired deferred work after an asset await with no commit", async () => {
    const fake = makeHost(draft({ intent: "edit", refs: [image()] }));
    let expired = false;
    const baseRead = fake.host.readAsset;
    fake.host.readAsset = async (id) => {
      const image = await baseRead(id);
      expired = true; // the request died while bytes were loading
      return image;
    };
    const actions = createApplicationActions(() => fake.host);
    const ctx: ActionContext = {
      assertActive() {
        if (expired) throw new McpError("REQUEST_EXPIRED", "expired");
      },
    };
    const err = await actions
      .dispatch(
        "workspace_patch",
        {
          expectedVersion: "inst:0",
          patch: { refs: [{ assetId: uuid() }] },
        },
        ctx,
      )
      .catch((e) => e);
    expect(code(err)).toBe("REQUEST_EXPIRED");
    expect(fake.commits).toHaveLength(0);
    expect(fake.submits).toHaveLength(0);
  });

  it("applies a combined patch as exactly one commit and version step", async () => {
    const primary = image();
    const fake = makeHost(
      draft({ intent: "edit", refs: [primary], baseId: primary.uid! }),
    );
    const actions = createApplicationActions(() => fake.host);
    const asset = uuid();
    const result = await actions.dispatch("workspace_patch", {
      expectedVersion: "inst:0",
      patch: {
        prompt: "<ref_image_0> redrawn",
        params: { quality: "high", outputCompression: 40 },
        refs: [{ uid: primary.uid }, { assetId: asset }],
      },
    });
    expect(fake.commits).toHaveLength(1);
    expect(fake.host.getVersion()).toBe("inst:1");
    const committed = fake.commits[0];
    expect(committed.prompt).toBe("<ref_image_0> redrawn");
    expect(committed.refs).toHaveLength(2);
    expect(committed.refs[0].uid).toBe(primary.uid);
    expect(committed.refs[1].assetId).toBe(asset);
    expect(committed.params.quality).toBe("high");
    const structured = result.structured as { version: string };
    expect(structured.version).toBe("inst:1");
  });

  it("protects the mask when the primary image would change", async () => {
    const primary = image();
    const second = image();
    const fake = makeHost(
      draft({
        intent: "edit",
        refs: [primary, second],
        baseId: primary.uid!,
        mask: { ...image(), name: "mask" },
      }),
    );
    const actions = createApplicationActions(() => fake.host);
    const err = await actions
      .dispatch("workspace_patch", {
        expectedVersion: "inst:0",
        patch: { primaryRefUid: second.uid },
      })
      .catch((e) => e);
    expect(code(err)).toBe("PRIMARY_MASK_CONFLICT");
    expect(fake.commits).toHaveLength(0);

    // Dropping the old primary through a refs-only replacement is still a
    // primary change, even though the fallback already points at the new one.
    const dropped = await actions
      .dispatch("workspace_patch", {
        expectedVersion: "inst:0",
        patch: { refs: [{ uid: second.uid }] },
      })
      .catch((e) => e);
    expect(code(dropped)).toBe("PRIMARY_MASK_CONFLICT");
    expect(fake.commits).toHaveLength(0);

    // Reordering references around the same primary keeps the same mask.
    await actions.dispatch("workspace_patch", {
      expectedVersion: "inst:0",
      patch: { refs: [{ uid: second.uid }, { uid: primary.uid }] },
    });
    expect(fake.commits).toHaveLength(1);
    const committed = fake.commits[0];
    expect(committed.baseId).toBe(primary.uid!);
    expect(committed.refs[0].uid).toBe(primary.uid!);
    expect(committed.refs[1].uid).toBe(second.uid!);
    expect(committed.mask?.name).toBe("mask");
  });
});

describe("task_submit idempotency", () => {
  it("returns the same task for a retried key and submits once", async () => {
    const fake = makeHost(draft());
    const actions = createApplicationActions(() => fake.host);
    const key = uuid();
    const first = (await actions.dispatch("task_submit", {
      expectedVersion: "inst:0",
      idempotencyKey: key,
    })) as { structured: { taskId: string; alreadySubmitted: boolean } };
    expect(first.structured.alreadySubmitted).toBe(false);
    const second = (await actions.dispatch("task_submit", {
      expectedVersion: "inst:0",
      idempotencyKey: key,
    })) as { structured: { taskId: string; alreadySubmitted: boolean } };
    expect(second.structured.taskId).toBe(first.structured.taskId);
    expect(second.structured.alreadySubmitted).toBe(true);
    expect(fake.submits).toHaveLength(1);
  });

  it("rejects the same key under a different workspace version", async () => {
    const fake = makeHost(draft());
    const actions = createApplicationActions(() => fake.host);
    const key = uuid();
    await actions.dispatch("task_submit", {
      expectedVersion: "inst:0",
      idempotencyKey: key,
    });
    fake.bump();
    const err = await actions
      .dispatch("task_submit", {
        expectedVersion: "inst:1",
        idempotencyKey: key,
      })
      .catch((e) => e);
    expect(code(err)).toBe("IDEMPOTENCY_CONFLICT");
    expect(fake.submits).toHaveLength(1);
  });
});
