import { expect, test } from "bun:test";
import { renderToStaticMarkup } from "react-dom/server";
import { AppError, cancelledError, isCancelledError } from "../../lib/api";
import { newDraft } from "../../lib/workspace";
import type { GenerationTask, ProviderStatus } from "../../lib/types";
import { GenerateFooter, PromptEditor } from "./Controls";

const status: ProviderStatus = {
  openrouter: true,
  bfl: true,
  comfy: false,
  runware: false,
  google: false,
  ark: false,
  byteplus: false,
  xai: false,
  qwencloud: false,
};

const draft = { ...newDraft(), prompt: "a lantern" };

const footer = {
  draft,
  onChange: () => {},
  providerStatus: status,
  busy: false,
  errors: [] as string[],
  finalPreview: "a lantern",
  onSettings: () => {},
  onGenerate: () => {},
  getVersion: () => "inst:1",
  applyOptimizedPrompt: () => {},
};

const waiting: GenerationTask = {
  family: "flux",
  intent: "create",
  modelId: "flux-3-image",
  provider: "openrouter",
  startedAt: Date.now(),
  total: 1,
  phase: "waiting",
};

test("idle generate footer is not busy and has no cancel", () => {
  const html = renderToStaticMarkup(<GenerateFooter {...footer} />);
  expect(html).not.toContain("aria-busy");
  expect(html).not.toContain("in-progress-cancel");
  expect(html).not.toContain('role="progressbar"');
  expect(html).not.toContain("skeleton");
});

test("in-progress generate uses busy button plus cancel, not a fake progress bar", () => {
  const html = renderToStaticMarkup(
    <GenerateFooter
      {...footer}
      generationTask={waiting}
      onCancelGeneration={() => {}}
    />,
  );
  expect(html).toContain("aria-busy");
  expect(html).toContain("activity-dot");
  expect(html).toContain("in-progress-cancel");
  expect(html).toContain("is-busy");
  expect(html).not.toContain('role="progressbar"');
  expect(html).not.toContain("skeleton");
});

test("generate failure landing keeps auth distinct from the idle submit row", () => {
  const html = renderToStaticMarkup(
    <GenerateFooter
      {...footer}
      generationFailure={{
        message: "OpenRouter rejected the API key",
        code: "backend_provider_unauthorized",
      }}
    />,
  );
  expect(html).toContain("generation-failure");
  expect(html).toContain("OpenRouter rejected the API key");
  expect(html).not.toContain("skeleton");
});

test("idle optimize is a button, not a waiting skeleton", () => {
  const html = renderToStaticMarkup(
    <PromptEditor
      draft={draft}
      onChange={() => {}}
      getVersion={() => "inst:1"}
      applyOptimizedPrompt={() => {}}
    />,
  );
  expect(html).toContain("prompt-optimize");
  expect(html).not.toContain("in-progress-cancel");
  expect(html).not.toContain("skeleton");
});

test("cancelled errors are silent", () => {
  expect(isCancelledError(cancelledError("backend_llm_cancelled"))).toBe(true);
  expect(isCancelledError(cancelledError())).toBe(true);
  expect(isCancelledError(new DOMException("Aborted", "AbortError"))).toBe(
    true,
  );
  expect(
    isCancelledError(
      new AppError("The API key is missing, invalid, or was rejected", {
        code: "backend_llm_unauthorized",
      }),
    ),
  ).toBe(false);
});
