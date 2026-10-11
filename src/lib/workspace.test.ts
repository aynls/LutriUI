import { expect, test } from "bun:test";
import {
  beginImageEdit,
  newDraft,
  reorderRefs,
  setPrimaryImage,
} from "./workspace";
import type { Draft, WorkingImage } from "./types";

function fixture(): Draft {
  return {
    ...newDraft(),
    provider: "bfl",
    canvas: { w: 1000, h: 500 },
    prompt: "Move <obj_1> from <ref_image_1> onto <ref_image_0>.",
    refs: [
      { uid: "a", dataUrl: "", width: 1000, height: 500, name: "base" },
      { uid: "b", dataUrl: "", width: 500, height: 1000, name: "source" },
    ],
    boxes: [
      {
        uid: "stable",
        id: "obj_1",
        role: "move",
        sourceId: "b",
        desc: "star",
        rect: { x: 500, y: 0, w: 500, h: 250 },
        srcRect: { x: 0, y: 500, w: 250, h: 500 },
      },
    ],
  };
}

test("重排参考图时同步改写标签，并保留来源身份", () => {
  const d = fixture();
  const next = reorderRefs(d, [d.refs[1], d.refs[0]]);
  expect(next.prompt).toBe(
    "Move <obj_1> from <ref_image_0> onto <ref_image_1>.",
  );
  expect(next.boxes[0].sourceId).toBe("b");
});

test("更换主图不会把已有蒙版挪到另一张图上", () => {
  const d = fixture();
  d.mask = { ...d.refs[0], name: "mask" };
  expect(() => setPrimaryImage(d, "b")).toThrow();
  expect(d.refs[0].uid).toBe("a");
});

const winner: WorkingImage = {
  uid: "incoming",
  assetId: "asset-winner",
  dataUrl: "data:image/png;winner",
  width: 32,
  height: 16,
  name: "winner",
};

test("beginImageEdit 把选定图设为主图并清空上一轮编辑，不带走蒙版和区域", () => {
  const d: Draft = {
    ...fixture(),
    intent: "edit",
    baseId: "a",
    prompt: "old instruction",
    mask: { uid: "m", dataUrl: "mask", width: 1000, height: 500, name: "mask" },
  };
  const kept = beginImageEdit(d, winner, true);
  expect(kept.intent).toBe("edit");
  expect(kept.prompt).toBe("");
  expect(kept.boxes).toEqual([]);
  expect(kept.mask).toBeNull();
  expect(kept.maskRects).toEqual([]);
  expect(kept.refs[0].assetId).toEqual("asset-winner");
  expect(kept.refs[0].dataUrl).toBe(winner.dataUrl);
  expect(kept.baseId).toBe(kept.refs[0].uid ?? null);
  expect(kept.refs.map((r) => r.name)).toEqual(["winner", "source"]);
  const dropped = beginImageEdit(d, winner, false);
  expect(dropped.refs).toHaveLength(1);
  expect(dropped.refs[0].assetId).toEqual("asset-winner");
});

test("beginImageEdit 不会把 Create 草稿的区域或提示词拷进 Edit", () => {
  const create = fixture();
  const next = beginImageEdit(create, winner, false);
  expect(next.intent).toBe("edit");
  expect(next.prompt).toBe("");
  expect(next.boxes).toEqual([]);
  expect(next.mask).toBeNull();
  expect(create.boxes).toHaveLength(1);
  expect(create.prompt).toContain("<obj_1>");
});
