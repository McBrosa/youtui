---
target: web/
total_score: 22
max_score: 40
na_heuristics: 
p0_count: 0
p1_count: 3
target_identity: "file:/Users/nrosa/projects/youtui/web/lib/youtui_web/live/player_live.ex"
target_fingerprint: "sha256:fa25b6587f85c4bf91b35019f53fda012b9fd17e28dd635ec55366b6c2cf85d3"
target_path: /Users/nrosa/projects/youtui/web/lib/youtui_web/live/player_live.ex
timestamp: 2026-10-01T16-02-49Z
slug: web-lib-youtui-web-live-player-live-ex
---
Method: dual-agent (A: `/root/luna_critique_design` · B: `/root/luna_critique_evidence`).

## Design verdict

The current interface is coherent, but its large promotional headline and identical result artwork feel generic. The local status, preparation controls, and queue provide the product-specific character.

Your Positivus reference is a useful direction: light surfaces, dark outlines, restrained offset shadows, and gold highlights. I recommend adapting that language to a compact video workspace, rather than importing the landing-page layout. This is based on your attached image; exact Figma fonts and tokens could not be verified.

## Design health

Scores judge the current interface: 0 = poor, 4 = excellent.

| Heuristic | Score | Main gap |
|---|---:|---|
| System status | 2 | Preparation lacks selected-title context and progress |
| Familiar language | 2 | “Local library,” “listening room,” and MP4 jargon |
| User control | 2 | No preparation cancellation or queue-clear undo |
| Consistency | 3 | Coherent controls, weaker narrow-screen behavior |
| Error prevention | 3 | Duplicate loads are guarded |
| Recognition | 2 | Identical artwork and unclear accessible states |
| Efficiency | 2 | Enter works; many repeated controls |
| Minimalist design | 3 | Clean layout, oversized persistent hero |
| Error recovery | 1 | Generic preparation failure gives no next step |
| Help | 2 | Useful empty-state guidance, limited contextual help |
| **Total** | **22/40** | **Acceptable local foundation** |

This is a design assessment, not a public-release sign-off.

## What works

- Desktop search, results, and player sit together.
- Native browser video controls avoid another custom interaction system.
- Queue guidance and the existing single-load guard reduce common mistakes.

## Priority issues

1. **P1 — Small, muted text and compact controls.** Metadata is around 10px, and the scan flagged contrast problems. This makes choosing videos harder. Increase readable secondary text, strengthen contrast, and enlarge small action targets. Gold controls should use dark text. See [result styles](/Users/nrosa/projects/youtui/web/assets/css/youtui.css:49). Suggested commands: `$impeccable typeset`, `$impeccable audit`.

2. **P1 — Missing accessible labels, focus, and toggle states.** The narrow-screen Search button loses its accessible name. The search field has no replacement focus indicator, and settings switches lack exposed on/off state. Add a persistent button label, visible focus, and semantic toggle state. See [search markup](/Users/nrosa/projects/youtui/web/lib/youtui_web/live/player_live.ex:486) and [focus styling](/Users/nrosa/projects/youtui/web/assets/css/youtui.css:25). Suggested command: `$impeccable harden`.

3. **P1 — Preparation and error recovery need clearer context.** During preparation, the player still says “Nothing playing”; the generic failure message gives no recovery step. Show the selected title, truthful preparation status, why other Play buttons are disabled, and a retry action. Preserve the one-job guard and cache reuse. See [preparation status](/Users/nrosa/projects/youtui/web/lib/youtui_web/live/player_live.ex:635). Suggested command: `$impeccable clarify`.

4. **P2 — The player sits below results on narrow screens.** Even three fixture results push it below the first viewport; the real default is 20. Keep active playback above the list or provide a clear jump to the player. See [responsive layout](/Users/nrosa/projects/youtui/web/assets/css/youtui.css:59). Suggested command: `$impeccable adapt`.

5. **P2 — Results lack visual recognition.** Every result uses the same play glyph, while titles truncate to one line. Use real video thumbnails where available, retain a fallback, and allow clearer titles. See [result artwork](/Users/nrosa/projects/youtui/web/assets/css/youtui.css:48). Suggested command: `$impeccable shape`.

## Cognitive load and emotional journey

The weak points are hierarchy after search and retaining the selected-video context during preparation. Three results expose six actions, but grouping two actions per row is sensible—not proof of critical overload.

The first screen feels calm. Search is straightforward. Preparation introduces uncertainty, and narrow-screen playback adds scrolling.

## Persona red flags

- **Alex, power user:** tiny metadata and repeated controls slow scanning.
- **Jordan, first-timer:** “Local library” implies saved content; preparation errors do not explain what to do.
- **Sam, keyboard/screen-reader user:** missing Search name, input focus, and switch state interrupt the main flow.

Minor copy fixes: use “Local session” instead of “Local library,” remove unnecessary yt-dlp/MP4 jargon, and use video-oriented language instead of “Your listening room.”

A useful design question: what if Play made the selected video and its preparation status the main focus immediately?

## Evidence limits

The rendered HTML detector produced **34 raw findings**: 19 text-size, 8 contrast, 3 glow, 2 padding, 1 tracking, and 1 gradient. These are not 34 distinct visible defects. Styles map chiefly to `youtui.css`; snapshot locations were line 0.

I excluded missing-stylesheet artifacts, hidden reconnect-alert findings, and detector-overlay self-occlusion. Browser inspection found no horizontal overflow at 1440px or 390px.

Preparation was observed, but actual fixture playback was not verified. The initial failure screenshot came from a fixture setup mistake, not a demonstrated playback regression. No UI or security settings changed.
