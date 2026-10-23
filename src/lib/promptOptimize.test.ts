import { expect, test } from "bun:test";
import { newDraft } from "./workspace";
import type { Draft } from "./types";
import { draftWithOptimizedPrompt, packOptimizeContext } from "./promptOptimize";

function draft(over: Partial<Draft> = {}): Draft {
  return {
    ...newDraft(),
    intent: "edit",
    family: "flux",
    modelId: "flux-3-image",
    provider: "bfl",
    prompt: "Move the lantern beside <ref_image_1>. Keep <obj_1>.",
    refs: [
      {
        uid: "main",
        dataUrl: "data:image/png;base64,AAAAsecret",
        width: 1024,
        height: 768,
        name: "scene.png",
        purpose: "subject",
      },
      {
        uid: "style",
        dataUrl: "data:image/jpeg;base64,BBBBsecret",
        width: 800,
        height: 600,
        name: "film.jpg",
        purpose: "style",
        note: "night grain",
      },
    ],
    baseId: "main",
    boxes: [
      {
        uid: "b1",
        id: "obj_1",
        role: "move",
        desc: "lantern",
        rect: { x: 0, y: 0, w: 10, h: 10 },
      },
    ],
    ...over,
  };
}

// 默认模型回落（空模型 → gpt-4.1-mini）与家族模板/单轮消息契约由
// src-tauri 的 llm.rs 内联测试和 tests/prompt_optimize.rs 在真实请求路径上覆盖；
// TS 侧仅保留打包上下文与版本守卫这两段独有行为。

test("packed context includes draft fields and never pixels or prior turns", () => {
  const text = packOptimizeContext(draft());
  expect(text).toContain("Task: edit");
  expect(text).toContain("Family: flux");
  expect(text).toContain("Provider route: bfl");
  expect(text).toContain("Edit main image: yes");
  expect(text).toContain("<ref_image_0>");
  expect(text).toContain("<ref_image_1>");
  expect(text).toContain("role=visual style, colors and lighting");
  expect(text).toContain("night grain");
  expect(text).toContain("<obj_1>");
  expect(text).toContain("Move the lantern");
  expect(text).not.toContain("data:image");
  expect(text).not.toContain("AAAAsecret");
  expect(text).not.toContain("BBBBsecret");
  expect(text).not.toContain('"role": "assistant"');
  expect(text.split("Current prompt:").length).toBe(2);
});

test("create drafts mark that there is no edit main image", () => {
  const text = packOptimizeContext(
    draft({
      intent: "create",
      refs: [
        {
          uid: "ref",
          dataUrl: "data:image/png;base64,CCCCsecret",
          width: 64,
          height: 64,
          name: "mood",
          purpose: "composition",
        },
      ],
      baseId: null,
      boxes: [],
    }),
  );
  expect(text).toContain("Task: create");
  expect(text).toContain("Edit main image: no");
  expect(text).toContain("role=composition and spatial arrangement");
  expect(text).not.toContain("CCCCsecret");
});

test("optimized prompt replaces text only while the workspace version matches", () => {
  const live = draft();
  const next = draftWithOptimizedPrompt(live, "inst:1", "inst:1", "rewritten");
  expect(next.prompt).toBe("rewritten");
  expect(next.family).toBe(live.family);
  expect(next.refs).toEqual(live.refs);
  expect(next.baseId).toBe(live.baseId);
  expect(draftWithOptimizedPrompt(live, "inst:1", "inst:1", live.prompt)).toBe(
    live,
  );
  expect(() =>
    draftWithOptimizedPrompt(live, "inst:2", "inst:1", "rewritten"),
  ).toThrow();
});
