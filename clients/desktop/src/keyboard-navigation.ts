export type SessionNavigationKey = "ArrowDown" | "ArrowUp" | "Home" | "End";

export function isSessionNavigationKey(key: string): key is SessionNavigationKey {
  return key === "ArrowDown" || key === "ArrowUp" || key === "Home" || key === "End";
}

export function nextSessionIndex(
  key: SessionNavigationKey,
  current: number,
  count: number,
): number {
  if (count <= 0) return -1;
  if (key === "Home") return 0;
  if (key === "End") return count - 1;
  const offset = key === "ArrowDown" ? 1 : -1;
  return (Math.max(0, current) + offset + count) % count;
}

export function focusFirstInteractive(root: ParentNode): HTMLElement | undefined {
  const target = root.querySelector<HTMLElement>(
    "button:not(:disabled), input:not(:disabled), select:not(:disabled), textarea:not(:disabled), [tabindex]:not([tabindex='-1'])",
  );
  target?.focus();
  return target ?? undefined;
}

export function restoreFocus(target: HTMLElement | null): void {
  if (target?.isConnected) target.focus();
}
