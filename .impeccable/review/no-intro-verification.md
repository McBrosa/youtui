# Intro removal

The user requested removal of the visible introductory heading and subtitle.

- Removed the heading section and its unused desktop and mobile styles.
- Preserved a hidden `youtui` page heading for screen readers.
- Moved the Enter hint into the existing search metadata.
- Added regression assertions to the workspace test.
- Updated the design record to describe the remaining typography roles.

`mix precommit` passed all 42 tests. Production assets and the Phoenix release built successfully. An independent Luna code review found no issue within scope.

Desktop and mobile captures confirm that search is the first visible main block, without the removed intro or horizontal overflow. Search names and keyboard focus remain intact. Native video controls, queue, and settings remain present.

A fixture-based browser check submitted one search with Enter and returned three rows. LiveView remained connected, with no visible connection alerts or browser errors. An earlier failed check used an incomplete synthesized Enter event and read hidden alerts as visible.

The updated companion is installed at `/Users/nrosa/lib/youtui/web`. Its files match the built release. An isolated installed-package check confirmed removal, authentication, assets, native controls, and clean shutdown.

The previous companion is recoverable at `/Users/nrosa/bin/.youtui-install.G30s3X/previous-web`. The launcher at `/Users/nrosa/bin/youtui` is unchanged. The existing user app was not stopped. Restart `youtui --web` to load the updated companion.

Captures: `no-intro-desktop.png` and `no-intro-mobile.png` in this directory. All test services and the isolated browser stopped after verification.
