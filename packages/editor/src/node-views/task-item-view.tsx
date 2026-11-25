import {
  type NodeViewComponentProps,
  useEditorEventCallback,
  useIsNodeSelected,
} from "@handlewithcare/react-prosemirror";
import type { NodeSpec } from "prosemirror-model";
import { forwardRef, type ReactNode } from "react";

import {
  createTaskStatusAttrs,
  getNextTaskStatus,
  normalizeTaskStatus,
} from "../tasks";
import { getSafeNodePos } from "./error-boundary";
import { TaskCheckbox } from "./task-checkbox";

export type AiLineSource = {
  t: string;
  s: string;
  e: string;
  a: number;
  b: number;
};

export function parseAiLineAttrs(element: HTMLElement): {
  aiId: string | null;
  src: AiLineSource[] | null;
} {
  const aiId = element.getAttribute("data-ai-id");
  let src: AiLineSource[] | null = null;
  const raw = element.getAttribute("data-src");
  if (raw) {
    try {
      const parsed = JSON.parse(raw);
      if (Array.isArray(parsed)) src = parsed;
    } catch {
      src = null;
    }
  }
  return { aiId: aiId || null, src };
}

/** DOM attributes for an AI line: `ai-line` class plus `data-ai-id`/`data-src`. */
export function aiLineDomAttrs(
  attrs: Record<string, unknown>,
): Record<string, string> {
  const aiId = attrs.aiId as string | null | undefined;
  if (!aiId) return {};
  const out: Record<string, string> = {
    class: "ai-line",
    "data-ai-id": aiId,
  };
  if (attrs.src) out["data-src"] = JSON.stringify(attrs.src);
  return out;
}

export const taskListNodeSpec: NodeSpec = {
  content: "taskItem+",
  group: "block",
  parseDOM: [{ tag: 'ul[data-type="taskList"]' }],
  toDOM() {
    return ["ul", { "data-type": "taskList", class: "task-list" }, 0];
  },
};

export const taskItemNodeSpec: NodeSpec = {
  content: "paragraph block*",
  defining: true,
  attrs: {
    status: { default: "todo" },
    checked: { default: false },
    taskId: { default: null },
    taskItemId: { default: null },
    aiId: { default: null },
    src: { default: null },
  },
  parseDOM: [
    {
      tag: 'li[data-type="taskItem"]',
      getAttrs(dom) {
        const element = dom as HTMLElement;
        const status = normalizeTaskStatus(
          element.getAttribute("data-status"),
          element.getAttribute("data-checked") === "true",
        );
        return {
          ...createTaskStatusAttrs(status),
          taskId: element.getAttribute("data-task-id"),
          taskItemId: element.getAttribute("data-task-item-id"),
          ...parseAiLineAttrs(element),
        };
      },
    },
  ],
  toDOM(node) {
    const status = normalizeTaskStatus(node.attrs.status, node.attrs.checked);
    return [
      "li",
      {
        "data-type": "taskItem",
        "data-status": status,
        "data-checked": status === "done" ? "true" : "false",
        "data-task-id": node.attrs.taskId,
        "data-task-item-id": node.attrs.taskItemId,
        ...aiLineDomAttrs(node.attrs),
      },
      0,
    ];
  },
};

export const TaskItemView = forwardRef<
  HTMLLIElement,
  NodeViewComponentProps & { children?: ReactNode }
>(function TaskItemView({ nodeProps, children, ...htmlAttrs }, ref) {
  const { node, getPos } = nodeProps;
  const status = normalizeTaskStatus(node.attrs.status, node.attrs.checked);
  const taskId = node.attrs.taskId as string | null;
  const aiId = (node.attrs.aiId as string | null | undefined) ?? undefined;
  const aiSrc = node.attrs.src ? JSON.stringify(node.attrs.src) : undefined;
  const isSelected = useIsNodeSelected();

  const handleToggle = useEditorEventCallback((view) => {
    if (!view) return;
    const pos = getSafeNodePos(getPos);
    if (pos === null) return;

    const nextStatus = getNextTaskStatus(status);
    const tr = view.state.tr.setNodeMarkup(pos, undefined, {
      ...node.attrs,
      ...createTaskStatusAttrs(nextStatus),
    });
    view.dispatch(tr);
  });

  return (
    <li
      ref={ref}
      {...htmlAttrs}
      className={
        aiId
          ? [htmlAttrs.className, "ai-line"].filter(Boolean).join(" ")
          : htmlAttrs.className
      }
      data-ai-id={aiId}
      data-src={aiSrc}
      data-type="taskItem"
      data-status={status}
      data-checked={status === "done" ? "true" : "false"}
      data-task-id={taskId ?? undefined}
      data-task-item-id={
        (node.attrs.taskItemId as string | null | undefined) ?? undefined
      }
    >
      <TaskCheckbox
        status={status}
        isInteractive
        isSelected={isSelected}
        onToggle={handleToggle}
      />
      <div ref={nodeProps.contentDOMRef}>{children}</div>
    </li>
  );
});
