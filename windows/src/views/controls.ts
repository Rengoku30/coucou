// Controls view — what is playing, the system volume, the microphone and the
// location switch. Windows only for now; there is no macOS view to port.
//
// The view asks Rust for the state once a second, and only while it is on
// screen: nothing runs when the island is closed. It updates its elements in
// place, so a refresh never interrupts a slider being dragged.

import { h, svg, clear } from "./dom";
import { ICONS } from "./icons";
import { Bridge, type SystemStatus } from "../core/bridge";
import type { ViewHost } from "./views";

const REFRESH_MS = 1000;

export function buildControls(): ViewHost {
  let status: SystemStatus | null = null;
  let lastRefresh = 0;
  let refreshing = false;
  let dragging = false;

  /** Runs a control, then reads the state back so the view shows what really happened. */
  const act = (change: Promise<unknown>) => void change.then(refresh);

  const title = h("span", { class: "ctl-title" });
  const artist = h("span", { class: "ctl-artist" });
  const previous = h("button", { class: "ctl-btn", title: "Previous", onclick: () => act(Bridge.mediaCommand("previous")) }, svg(ICONS.previous, 12));
  const toggle = h("button", { class: "ctl-btn", title: "Play or pause", onclick: () => act(Bridge.mediaCommand("toggle")) });
  const next = h("button", { class: "ctl-btn", title: "Next", onclick: () => act(Bridge.mediaCommand("next")) }, svg(ICONS.next, 12));

  const mute = h("button", {
    class: "ctl-btn",
    title: "Mute",
    onclick: () => status && act(Bridge.setOutputMuted(!status.muted)),
  });
  const volume = h("input", {
    type: "range", min: "0", max: "1", step: "0.01", class: "ctl-volume", title: "Volume",
    oninput: () => void Bridge.setVolume(Number(volume.value)),
    onpointerdown: () => { dragging = true; },
    onchange: () => { dragging = false; void refresh(); },
  }) as HTMLInputElement;

  const mic = h("button", {
    class: "switch",
    title: "Microphone",
    onclick: () => status?.micMuted != null && act(Bridge.setMicMuted(!status.micMuted)),
  });
  const micLabel = h("span", {});
  const location = h("button", {
    class: "switch",
    title: "Location",
    onclick: () => status && act(Bridge.setLocation(status.location !== "on")),
  });
  const locationLabel = h("span", {});
  // The PC-wide switch needs an administrator, so all we can do is open it.
  const locationSettings = h("button", {
    class: "link-btn",
    style: "color:#8e939c;font-size:11.5px",
    text: "Open Settings",
    onclick: () => void Bridge.openLocationSettings(),
  });

  const rows = h(
    "div",
    { class: "settings-rows" },
    h("div", { class: "settings-row" }, svg(ICONS.note, 12), h("div", { class: "ctl-track" }, title, artist), previous, toggle, next),
    h("div", { class: "settings-row" }, mute, volume),
    h("div", { class: "settings-row" }, svg(ICONS.mic, 12), micLabel, mic, h("div", { class: "grow" }), svg(ICONS.pin, 12), locationLabel, location, locationSettings),
  );
  const unavailable = h("div", { class: "sub", text: "Controls are only available on Windows for now." });

  const el = h("div", { class: "view" },
    h("div", { class: "card" }, h("div", { class: "stack", style: "padding:14px 16px 14px 84px" }, rows, unavailable)));

  async function refresh() {
    if (refreshing) return;
    refreshing = true;
    const latest = await Bridge.systemStatus();
    refreshing = false;
    if (latest) status = latest;
    paint();
  }

  function paint() {
    const ready = status?.supported ?? false;
    rows.style.display = ready ? "" : "none";
    unavailable.style.display = ready ? "none" : "";
    if (!status || !ready) return;

    const media = status.media;
    title.textContent = media ? media.title || "Unknown title" : "Nothing playing";
    artist.textContent = media?.artist ?? "";
    for (const b of [previous, toggle, next]) b.toggleAttribute("disabled", !media);
    clear(toggle);
    toggle.append(svg(media?.playing ? ICONS.pause : ICONS.play, 12));

    const hasSpeaker = status.volume != null;
    mute.toggleAttribute("disabled", !hasSpeaker);
    volume.toggleAttribute("disabled", !hasSpeaker);
    clear(mute);
    mute.append(svg(status.muted ? ICONS.speakerOff : ICONS.speakerOn, 14));
    if (!dragging) volume.value = String(status.volume ?? 0);
    volume.style.opacity = status.muted ? "0.4" : "1";

    const hasMic = status.micMuted != null;
    mic.toggleAttribute("disabled", !hasMic);
    mic.classList.toggle("on", status.micMuted === false);
    micLabel.textContent = !hasMic ? "No mic" : status.micMuted ? "Mic off" : "Mic on";

    const pcWideOff = status.location === "systemOff";
    location.style.display = pcWideOff ? "none" : "";
    locationSettings.style.display = pcWideOff ? "" : "none";
    location.classList.toggle("on", status.location === "on");
    locationLabel.textContent = pcWideOff ? "Location off for this PC" : status.location === "on" ? "Location on" : "Location off";
  }

  paint();
  return {
    el,
    sync: paint,
    tick(nowMs: number) {
      if (nowMs - lastRefresh < REFRESH_MS) return;
      lastRefresh = nowMs;
      void refresh();
    },
  };
}
