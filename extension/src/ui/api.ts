// Popup / approval -> background calls. Keys never come back over this channel.

export async function call<T = any>(type: string, payload: Record<string, unknown> = {}): Promise<T> {
  const r = await chrome.runtime.sendMessage({ channel: "pearl-wallet:ui", type, ...payload });
  if (!r) throw new Error("no response from the wallet background");
  if (!r.ok) throw new Error(r.error);
  return r.result as T;
}

export function applyTheme(theme: "system" | "light" | "dark") {
  const el = document.documentElement;
  if (theme === "system") el.removeAttribute("data-theme");
  else el.setAttribute("data-theme", theme);
  try {
    localStorage.setItem("theme", theme);
  } catch {
    /* storage blocked */
  }
}

export function initialTheme() {
  try {
    const t = localStorage.getItem("theme");
    if (t === "light" || t === "dark") document.documentElement.setAttribute("data-theme", t);
  } catch {
    /* storage blocked */
  }
}

export async function copy(text: string) {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    return false;
  }
}
