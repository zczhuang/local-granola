import type { ReactNode } from "react";

export function SettingsPageTitle({ title }: { title: ReactNode }) {
  return (
    <h2 className="font-sans text-2xl leading-tight font-semibold tracking-tight">
      {title}
    </h2>
  );
}
