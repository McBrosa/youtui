---
version: 1
slug: "web-lib-youtui-web-live-player-live-ex"
primary_target: "web/lib/youtui_web/live/player_live.ex"
related_targets: ["web/assets/css/youtui.css"]
---

# Local browser player

Mode: Operate. Scope: the approved light-and-gold restyle, the three P1 critique fixes, contained result cards, and an optional Theater mode. Preserve search, queues, settings, native video controls, session ownership, prepared-video reuse, and the global preparation lock. Thumbnails remain deferred.

## Direction contract

THESIS: A local video workspace makes search, the selected video, and preparation status readable together. It refuses a marketing page inside the player.

OWN-WORLD: The user's Positivus reference pins light surfaces, dark outlines, gold highlights, and restrained offset shadows. Use near-white, white, ink, and readable gold with dark text. No green accent, glow, or decorative gradients. Keep the existing typography families with a readable UI scale.

STORY: Search, choose Play or Add to queue, see which video prepares, then use native playback controls. A failed preparation offers a guarded retry.

FIRST VIEWPORT: A compact brand/status header, full-width search as the first visible main block, and the existing results/player columns by default. The user's follow-up removes the introductory heading and subtitle. Keep the Enter hint in search metadata and a hidden page heading for screen readers. Gold marks the primary controls and selected preparation state. Secondary text and control targets remain readable at narrow widths. Long titles and channel names must not widen result cards or obscure Play buttons.

FORM: User-pinned Positivus utility adaptation overrides open identity exploration; seed 2e22f71a is acknowledged. The default composition is retained. Theater mode spans the player across the workspace and places results below it. The same video element stays mounted, so toggling the layout does not restart playback or prepare another video. The mode belongs to the current tab and starts disabled. Code-led execution for the requested Luna workers; no workflow preference is stored. Signature interaction: Play visibly locks preparation once, names the selected video, and restores controls on completion or failure. Reduced motion disables decorative transitions.

FINISH: unreviewed and undocumented is unfinished; this build ends with the finish review, the verdict, DESIGN.md, and every shipping raster carrying its provenance

## Verification

Inspect normal and Theater mode desktop and 390px captures in one batch. Verify names, focus, toggle states, contrast, small targets, error retry, rapid duplicate clicks, and cached replay. Verify long-title containment and coordinate-based Play clicks at desktop, breakpoint, and narrow widths. Verify that Theater mode preserves the video element and preparation state. Keep access on loopback. A fresh finish reviewer checks the render and changes before local reinstall.
