# Result containment and Theater mode verification

The requested changes preserve the existing light-and-gold interface and loopback-only access.

## Automated checks

- `mix precommit`: 44 tests passed, including two new Theater mode tests.
- The toggle tests cover search, queue, ownership, playback, and active preparation state.
- The Impeccable layout detector returned no findings before or after the changes.
- Production assets and the Phoenix release built successfully.

## Browser checks

A Luna worker used an isolated production release, a mock downloader, and a valid local MP4 fixture. The fixture results are synthetic.

Normal layout passed at 1440, 1280, 1024, 901, 900, 768, 720, 621, 620, 390, and 320 CSS pixels. The 720px viewport approximates the available width at 200% zoom on a 1440px display. It is not a browser zoom test.

At each width, all 20 result cards stayed within the results column. All 40 Play and Add controls passed pointer-hit checks and had targets of at least 44×44px. Long titles and channel names truncated. The document had no horizontal overflow.

A coordinate-based Play click on the unbroken-title row showed the selected preparation state. The native video reached `readyState=4` after one preparation.

Theater mode used the full 1256px workspace width on desktop. Results, the queue, and settings appeared below the player. The 390px Theater mode layout also passed ordering and pointer-hit checks.

Pointer toggles on and off preserved the video element, source, and single preparation count. Both modes retained the queue and settings. Neither mode entered browser fullscreen.

The worker stopped its launcher, Chrome instance, and named browser daemon. Test ports 4113 and 9242 were free afterward. The user's running session was not stopped.

## Captures

The root agent inspected these full-page captures together. No further interface edits followed the capture pass.

- `overlap-fixed-desktop.png`: 1440×2370.
- `overlap-fixed-mobile.png`: 390×2909.
- `theater-fixed-desktop.png`: 1440×3169.
- `theater-fixed-mobile.png`: 390×2909.

These files are verification artifacts, not shipping image assets. The video shows synthetic test media, not a downloaded YouTube clip.

## Review and local installation

The independent Luna finish review returned `disposition: ship` with no material fixes.

The production companion was installed at `/Users/nrosa/lib/youtui/web`. The existing `/Users/nrosa/bin/youtui` launcher already matched the built Rust binary and did not require replacement. A recursive comparison found no differences between the built and installed companion.

The previous companion remains recoverable at `/Users/nrosa/bin/.youtui-install.uAJA1n/previous-web`.

An isolated installed-launcher test passed companion discovery, authenticated startup, light-and-gold assets, native controls, Theater mode markup, and switch values. Unauthenticated access returned 401. The launcher shut down cleanly and released port 4112.

The user's existing process remained active. Restart `youtui --web` to load the new interface.
