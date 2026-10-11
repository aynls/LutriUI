import templates from "../../shared/prompt-optimize.json";
import { familyById, modelById, pickParams, routeFor } from "../models/catalog";
import { promptLength } from "../models";
import { regionsEnabled } from "../models/flux/layout";
import type { Draft, FamilyId } from "./types";

export const DEFAULT_LLM_MODEL = "gpt-4.1-mini";

const PURPOSE: Record<string, string> = {
  reference: "general reference",
  style: "visual style, colors and lighting",
  subject: "subject identity and appearance",
  composition: "composition and spatial arrangement",
  custom: "custom reference",
};

/** Pack the current draft into one independent user message. Never includes pixels, data URLs, or prior turns. */
export function packOptimizeContext(d: Draft): string {
  const family = familyById(d.family);
  const model = modelById(d.modelId);
  const route = routeFor(d);
  const params = pickParams(d.modelId, d.provider, d.params);
  const budget = promptLength(d);
  const lines: string[] = [
    `Task: ${d.intent}`,
    `Family: ${d.family} (${family.label})`,
    `Model: ${d.modelId}${model && model.label !== d.modelId ? ` (${model.label})` : ""}`,
    `Provider route: ${d.provider}${route?.model ? ` → ${route.model}` : ""}`,
  ];
  const paramLines = Object.entries(params).map(([key, value]) => `- ${key}: ${stringify(value)}`);
  lines.push("Parameters:");
  lines.push(paramLines.length ? paramLines.join("\n") : "- (none beyond model defaults)");
  lines.push(
    `Prompt character budget: compiled length counts toward max ${budget.max}; current compiled length is ${budget.length}.`,
  );
  lines.push("Current prompt:");
  lines.push(`<<<\n${d.prompt}\n>>>`);
  if (d.intent === "edit" && d.refs[0]) {
    const main = d.refs[0];
    lines.push(
      [
        "Edit main image: yes",
        `  index: 0`,
        `  tag: <ref_image_0>`,
        `  name: ${main.name || "(unnamed)"}`,
        `  size: ${main.width}x${main.height}`,
        main.purpose ? `  purpose: ${PURPOSE[main.purpose] ?? main.purpose}` : "",
        main.note?.trim() ? `  note: ${main.note.trim()}` : "",
      ]
        .filter(Boolean)
        .join("\n"),
    );
  } else if (d.intent === "edit") {
    lines.push("Edit main image: missing");
  } else {
    lines.push("Edit main image: no (create a new image)");
  }
  if (d.refs.length) {
    lines.push("References:");
    d.refs.forEach((ref, i) => {
      const role =
        d.intent === "edit" && i === 0
          ? "edit-main"
          : (PURPOSE[ref.purpose ?? "reference"] ?? ref.purpose ?? "reference");
      const note = ref.note?.trim() ? ` note=${ref.note.trim()}` : "";
      lines.push(
        `- <ref_image_${i}> name=${ref.name || "(unnamed)"} size=${ref.width}x${ref.height} role=${role}${note}`,
      );
    });
  } else {
    lines.push("References: none");
  }
  lines.push(d.mask ? "Mask: present (pixels not included)" : "Mask: none");
  if (d.family === "flux") {
    lines.push(`Regions enabled: ${regionsEnabled(d) ? "yes" : "no (kept in draft, not sent)"}`);
  }
  if (d.boxes.length) {
    lines.push(regionsEnabled(d) ? "Regions (sent with request):" : "Regions (draft only, not sent):");
    for (const box of d.boxes) {
      const desc = box.desc.trim() ? ` desc=${box.desc.trim()}` : "";
      lines.push(`- <${box.id}> role=${box.role}${desc}`);
    }
  }
  return lines.join("\n");
}

function stringify(value: unknown): string {
  if (typeof value === "string") return value;
  if (typeof value === "number" || typeof value === "boolean") return String(value);
  return JSON.stringify(value);
}

export function familyTemplate(family: FamilyId | string): string[] {
  const overlay = (templates.families as Record<string, string[] | undefined>)[family];
  return overlay?.length ? overlay : [];
}

export function genericTemplate(): string[] {
  return templates.generic;
}

/** Documents the Chat Completions payload shape. The desktop backend builds the live request. */
export function optimizeChatMessages(family: FamilyId | string, userContent: string) {
  const extra = familyTemplate(family);
  const system = [...genericTemplate(), ...extra].join("\n\n");
  return [
    { role: "system" as const, content: system },
    { role: "user" as const, content: userContent },
  ];
}
