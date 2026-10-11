import { expect, test } from "bun:test";
import { catalog } from "../models/catalog";
import { newDraft } from "./workspace";
import type { Draft } from "./types";
import {
  familyTemplate,
  genericTemplate,
  optimizeChatMessages,
  packOptimizeContext,
} from "./promptOptimize";

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

test("every catalog family has a prompt-optimize overlay", () => {
  for (const family of catalog.families) {
    expect(familyTemplate(family.id).join(" ")).not.toBe("");
  }
  expect(familyTemplate("not-a-family")).toEqual([]);
  expect(genericTemplate().join(" ")).toContain("independent");
});

test("family messages stay a single independent turn", () => {
  const user = packOptimizeContext(draft());
  const messages = optimizeChatMessages("flux", user);
  expect(messages).toHaveLength(2);
  expect(messages[0].role).toBe("system");
  expect(messages[1].role).toBe("user");
  expect(messages[0].content).toContain("FLUX.3 Image");
  expect(messages[1].content).toBe(user);
  expect(optimizeChatMessages("unknown-family", user)[0].content).not.toContain(
    "FLUX.3 Image",
  );
  expect(optimizeChatMessages("gpt", user)[0].content).toContain("GPT Image");
});

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
