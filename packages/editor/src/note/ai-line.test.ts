// @vitest-environment jsdom

import { DOMParser as PMDOMParser, DOMSerializer } from "prosemirror-model";
import { describe, expect, it } from "vitest";

import { schema } from "./schema";

const src = [{ t: "t1", s: "w1", e: "w3", a: 0, b: 900 }];

const docWithAiLine = {
  type: "doc",
  content: [
    {
      type: "bulletList",
      content: [
        {
          type: "listItem",
          attrs: { aiId: "ai-1", src },
          content: [
            { type: "paragraph", content: [{ type: "text", text: "AI line" }] },
          ],
        },
      ],
    },
    {
      type: "taskList",
      content: [
        {
          type: "taskItem",
          attrs: { aiId: "ai-2", src },
          content: [
            { type: "paragraph", content: [{ type: "text", text: "Do it" }] },
          ],
        },
      ],
    },
  ],
};

describe("ai-line attrs", () => {
  it("keeps existing documents valid with null defaults", () => {
    const doc = schema.nodeFromJSON({
      type: "doc",
      content: [
        {
          type: "bulletList",
          content: [
            {
              type: "listItem",
              content: [{ type: "paragraph" }],
            },
          ],
        },
      ],
    });
    doc.check();
    const item = doc.firstChild!.firstChild!;
    expect(item.attrs.aiId).toBeNull();
    expect(item.attrs.src).toBeNull();
  });

  it("accepts aiId and src on listItem and taskItem", () => {
    const doc = schema.nodeFromJSON(docWithAiLine);
    doc.check();
    expect(doc.firstChild!.firstChild!.attrs).toEqual({ aiId: "ai-1", src });
    expect(doc.child(1).firstChild!.attrs.aiId).toBe("ai-2");
  });

  it("serializes to DOM with ai-line class, data-ai-id and data-src", () => {
    const doc = schema.nodeFromJSON(docWithAiLine);
    const fragment = DOMSerializer.fromSchema(schema).serializeFragment(
      doc.content,
    );
    const container = document.createElement("div");
    container.appendChild(fragment);
    const li = container.querySelector("li.ai-line")!;
    expect(li.getAttribute("data-ai-id")).toBe("ai-1");
    expect(JSON.parse(li.getAttribute("data-src")!)).toEqual(src);
    const task = container.querySelector('li[data-type="taskItem"]')!;
    expect(task.classList.contains("ai-line")).toBe(true);
  });

  it("does not add ai-line markup to plain list items", () => {
    const doc = schema.nodeFromJSON({
      type: "doc",
      content: [
        {
          type: "bulletList",
          content: [{ type: "listItem", content: [{ type: "paragraph" }] }],
        },
      ],
    });
    const container = document.createElement("div");
    container.appendChild(
      DOMSerializer.fromSchema(schema).serializeFragment(doc.content),
    );
    expect(container.querySelector("li")!.className).toBe("");
    expect(container.querySelector("li")!.hasAttribute("data-ai-id")).toBe(
      false,
    );
  });

  it("round-trips through parseDOM", () => {
    const doc = schema.nodeFromJSON(docWithAiLine);
    const container = document.createElement("div");
    container.appendChild(
      DOMSerializer.fromSchema(schema).serializeFragment(doc.content),
    );
    const parsed = PMDOMParser.fromSchema(schema).parse(container);
    expect(parsed.firstChild!.firstChild!.attrs).toEqual({
      aiId: "ai-1",
      src,
    });
    expect(parsed.child(1).firstChild!.attrs.src).toEqual(src);
  });
});
