import { useLingui } from "@lingui/react/macro";
import { platform } from "@tauri-apps/plugin-os";
import type { ReactNode } from "react";

import {
  CalendarBlank,
  FileText,
  FolderSimple,
  GearSix,
  Notebook,
  Users,
} from "@anlg/ui/components/icons";
import { cn } from "@anlg/utils";

import { useSidebarNotes } from "./note-filter";

import {
  leaveAllOverlayTabs,
  leaveOverlayTab,
} from "~/shared/leave-overlay-tab";
import { useTabs } from "~/store/zustand/tabs";

type SidebarDestination =
  | "notes"
  | "calendar"
  | "folders"
  | "contacts"
  | "templates"
  | "settings";

const OVERLAY_TAB_TYPES = new Set([
  "calendar",
  "folders",
  "contacts",
  "templates",
  "automations",
  "settings",
]);

export function SidebarIconBar() {
  const { t } = useLingui();
  const currentTab = useTabs((state) => state.currentTab);
  const openNew = useTabs((state) => state.openNew);
  const settingsShortcut = platform() === "macos" ? "⌘," : "Ctrl+,";
  const isInOverlayTab = Boolean(
    currentTab?.type && OVERLAY_TAB_TYPES.has(currentTab.type),
  );

  const handleSelect = (destination: SidebarDestination) => {
    if (destination === "notes") {
      useSidebarNotes.getState().setView("mine", null);
      if (isInOverlayTab) {
        leaveAllOverlayTabs();
      }
      return;
    }

    if (currentTab?.type === destination) {
      leaveOverlayTab();
      return;
    }

    if (destination === "settings") {
      openNew?.({ type: "settings", state: { tab: "app" } });
      return;
    }

    openNew?.({ type: destination });
  };

  const items: Array<{
    id: SidebarDestination;
    label: string;
    title: string;
    icon: ReactNode;
  }> = [
    {
      id: "notes",
      label: t`Notes`,
      title: t`Notes`,
      icon: <Notebook size={16} aria-hidden="true" />,
    },
    {
      id: "calendar",
      label: t`Calendar`,
      title: t`Calendar`,
      icon: <CalendarBlank size={16} aria-hidden="true" />,
    },
    {
      id: "folders",
      label: t`Folders`,
      title: t`Folders`,
      icon: <FolderSimple size={16} aria-hidden="true" />,
    },
    {
      id: "contacts",
      label: t`Contacts`,
      title: t`Contacts`,
      icon: <Users size={16} aria-hidden="true" />,
    },
    {
      id: "templates",
      label: t`Templates`,
      title: t`Templates`,
      icon: <FileText size={16} aria-hidden="true" />,
    },
    {
      id: "settings",
      label: t`Settings`,
      title: `${t`Settings`} (${settingsShortcut})`,
      icon: <GearSix size={16} aria-hidden="true" />,
    },
  ];

  return (
    <nav
      aria-label={t`Workspace navigation`}
      data-testid="sidebar-icon-bar"
      data-tauri-drag-region="false"
      className="border-border/60 flex shrink-0 items-center justify-between border-t px-1.5 py-1.5"
    >
      {items.map((item) => {
        const isActive =
          item.id === "notes"
            ? !isInOverlayTab
            : currentTab?.type === item.id;
        return (
          <button
            key={item.id}
            type="button"
            aria-label={item.label}
            aria-current={isActive ? "page" : undefined}
            title={item.title}
            data-tauri-drag-region="false"
            onClick={() => handleSelect(item.id)}
            className={cn([
              "flex size-7 items-center justify-center rounded-lg transition-colors",
              "focus-visible:ring-ring focus-visible:ring-2 focus-visible:outline-hidden",
              isActive
                ? "bg-sidebar-accent text-foreground"
                : "text-muted-foreground hover:bg-sidebar-accent/60 hover:text-foreground",
            ])}
          >
            {item.icon}
          </button>
        );
      })}
    </nav>
  );
}
