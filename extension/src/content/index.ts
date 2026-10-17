// Content script (isolated world): relays page <-> background. The page cannot
// talk to the background directly, and the background reads the origin from the
// sender, so a page cannot claim to be another site.

const PAGE = "pearl-wallet:page";
const CONTENT = "pearl-wallet:content";

window.addEventListener("message", (ev) => {
  if (ev.source !== window || ev.data?.source !== PAGE) return;
  const { id, method, params } = ev.data;
  chrome.runtime
    .sendMessage({ channel: "pearl-wallet:dapp", method, params })
    .then((r) => window.postMessage({ source: CONTENT, id, ...(r ?? { ok: false, error: "no response" }) }, window.location.origin))
    .catch((e) => window.postMessage({ source: CONTENT, id, ok: false, error: String(e?.message ?? e) }, window.location.origin));
});

chrome.runtime.onMessage.addListener((msg) => {
  if (msg?.channel === "pearl-wallet:event") window.postMessage({ source: CONTENT, event: msg.event, data: msg }, window.location.origin);
});
