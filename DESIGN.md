---
name: Youtui Web
description: A local video workspace for search, queue management, and browser playback.
colors:
  page: "#f7f7f2"
  surface: "#fff"
  surface-soft: "#f1f1eb"
  ink: "#191a23"
  text: "#30313a"
  muted: "#5c5d66"
  subtle: "#74757d"
  line: "#c9c9c3"
  gold: "#e7bc45"
  gold-soft: "#fff2c9"
  gold-ink: "#654b05"
  error: "#8f2d26"
  error-bg: "#fff0ed"
typography:
  brand:
    fontFamily: "Manrope, sans-serif"
    fontSize: "1.2rem"
    fontWeight: 800
    lineHeight: "normal"
    letterSpacing: "-0.03em"
  title:
    fontFamily: "Manrope, sans-serif"
    fontSize: "1.05rem"
    fontWeight: 650
    lineHeight: "normal"
    letterSpacing: "-0.02em"
  body:
    fontFamily: "DM Sans, sans-serif"
    fontSize: "16px"
    fontWeight: 400
    lineHeight: 1.5
  action:
    fontFamily: "DM Sans, sans-serif"
    fontSize: "0.95rem"
    fontWeight: 700
    lineHeight: 1.5
rounded:
  control: "8px"
  card: "12px"
  pill: "99px"
spacing:
  compact: "0.55rem"
  control: "0.75rem"
  card: "0.85rem"
  section: "1rem"
components:
  button-primary:
    backgroundColor: "{colors.gold}"
    textColor: "{colors.ink}"
    typography: "{typography.action}"
    rounded: "{rounded.control}"
    padding: "0 1.1rem"
    height: "44px"
  button-icon-secondary:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.ink}"
    rounded: "{rounded.control}"
    size: "44px"
  input-search:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.ink}"
    rounded: "{rounded.control}"
    padding: "0 1rem"
    height: "52px"
  card-player:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.ink}"
    rounded: "{rounded.card}"
    padding: "0.85rem"
  chip-count:
    backgroundColor: "{colors.gold}"
    textColor: "{colors.ink}"
    rounded: "7px"
    size: "24px"
---

# Design System: Youtui Web

## Overview

**Creative North Star: "Positivus-inspired light-and-gold workspace"**

Youtui is a local video workspace. Its browser interface keeps search, results, the player, and queue visible as parts of one utility. It adapts the user's Positivus reference with light surfaces, dark outlines, gold highlights, and restrained offset shadows. It is not a marketing landing page.

DM Sans carries interface text and Manrope marks the brand and headings. The layout keeps the results beside the player on wide screens, then stacks the workspace on narrow screens. The interface names preparation and error states next to the player and keeps browser video controls native.

**Key Characteristics:**

- Light paper-like page with white work surfaces.
- Dark outlines and gold controls provide clear boundaries and actions.
- Search, results, playback, queue, and settings share one workspace.

## Colors

The palette pairs a warm near-white canvas with ink text, neutral dividers, gold actions, and a distinct red error state.

### Primary

- **Workspace Gold**: Primary controls, status marks, and selected preparation states use gold with dark text.
- **Gold Wash**: Soft gold marks secondary selected surfaces and icon backgrounds.
- **Gold Ink**: Dark gold text appears on gold-tinted surfaces and text actions.

### Tertiary

- **Error Red**: Error text and borders use a dark red.
- **Error Wash**: Error messages sit on a pale red surface.

### Neutral

- **Warm Page**: The page canvas uses a warm near-white.
- **White Surface**: Cards, controls, and panels use white.
- **Soft Surface**: Quiet backgrounds and scrollbar tracks use a pale warm gray.
- **Ink**: Headings, primary text, and outlines use a deep blue-black.
- **Body Text**: Secondary foreground text uses dark charcoal.
- **Muted Text**: Supporting text, metadata, and placeholders use a readable gray.
- **Subtle Text**: A lighter secondary gray is available for low-priority labels.
- **Divider**: The header rule and quiet outlines use a mid-light gray.

## Typography

**Brand and Heading Font:** Manrope (with sans-serif fallback)  
**Body Font:** DM Sans (with sans-serif fallback)

**Character:** Manrope gives the brand and headings a compact, strong shape. DM Sans keeps controls, metadata, and longer interface text readable.

### Hierarchy

- **Brand** (800, 1.2rem): The top-bar name uses Manrope. The page has no visible introductory heading.
- **Title** (650, 1.05rem): Section and card headings use Manrope.
- **Body** (400, 16px, 1.5 line-height): Default interface text uses DM Sans.
- **Label** (600–700, 0.75rem–0.95rem): Controls, counts, and compact metadata use DM Sans.

## Layout

The header spans the viewport. Main content has a 1400px maximum width and responsive horizontal padding from 1rem to 5rem. On wide screens, the workspace uses two columns. Results occupy the left column. The player sits above the queue and settings in the right column.

Theater mode places the player across both columns, with results and the side panels below it. The theater player is capped at the smaller of 70vh and 720px, with a 200px minimum height. At 900px, the workspace becomes one column. Theater mode keeps the player first, followed by results and the side panels.

At 620px, the side content stacks and the local-session label and keyboard hint hide. Search and result actions become compact but retain 44px control targets. The theater player returns to 16:9.

Spacing comes from repeated gaps and padding between about 0.35rem and 1.5rem. Cards use compact internal padding, while larger gaps separate major workspace areas. Result cards remain compact and keep titles and channel names on one line with ellipsis.

**The Result Containment Rule.** The results list and each card can shrink within their grid tracks. Text truncates before the artwork or action buttons shrink.

## Elevation & Depth

Depth uses dark outlines as the primary boundary, with soft offset shadows on search, player, queue, settings, and hovered results. Shadows have a visible offset and blur. Gold fills mark interactive or selected elements rather than acting as a glow.

### Shadow Vocabulary

- **Panel shadow** (`0 4px 12px #191a2310, 0 2px 0 #191a2310`): Search, player, queue, and settings panels.
- **Result resting shadow** (`0 2px 7px #191a2309`): Result cards at rest.
- **Result hover shadow** (`0 4px 11px #191a2314, 0 2px 0 #191a230c`): Result cards under pointer hover.

## Shapes

Controls use gently rounded corners, usually 7–8px. Cards use 11–13px corners. Status marks, switches, and scrollbar thumbs use pill shapes. Dark 1px outlines define controls and cards. Empty and loading states use dashed outlines.

The default player surface is clipped to 16:9 with a dark background. Theater mode removes that ratio and uses a capped-height surface on wider screens. At 620px and below it returns to 16:9.

## Components

### Buttons

Gold action buttons use a dark outline, dark text, 8px corners, and at least 44px height. Their label uses DM Sans at 0.95rem and weight 700. Hover lightens the gold and lifts the control by 1px. Disabled controls reduce opacity and remove the lift. Icon-only secondary buttons use a white fill, dark outline, and a 44px square target.

### Theater Toggle

The theater toggle sits beside the player status badge. It has an 8px corner, dark outline, and at least 44px height. It is white at rest, uses the soft gold fill on hover, and uses workspace gold when `aria-pressed="true"`. Keyboard focus uses the shared 3px gold-brown outline.

### Chips

The queue count uses a compact gold badge with dark text, a dark outline, and 7px corners. It stays inline with the queue heading.

### Cards / Containers

Player, queue, and settings panels use white fills, dark outlines, 12px corners, soft offset shadows, and 0.85rem padding. The search panel has a 1px outline and 13px corners. Results use a compact 11px card with a subtle resting shadow that strengthens on hover.

### Inputs / Fields

The search field uses a white fill, 52px height, 8px corners, and no visible resting border inside its outlined search panel. Placeholder text uses the muted color. Keyboard focus uses a 3px gold-brown outline with 1px offset.

### Navigation

The top bar uses a white surface, a bottom divider, and 68px minimum height. On small screens it uses 60px height and 1rem horizontal padding. The brand uses bold Manrope and a gold square play mark. Backend status and local-session text sit at the opposite edge.

### Preparation and Error States

Preparation appears in a soft gold panel below the player and names the selected video. A spinner marks active work. Errors use a pale red panel with dark red text and border; a gold Retry button appears when preparation can restart.

### Settings Switch

Settings rows use full-width transparent controls with bottom dividers. The switch has a dark outline and pill track; enabled state uses gold and moves the dark thumb to the right.

## Do's and Don'ts

### Do:

- **Do** keep the interface light, with dark outlines and gold action emphasis.
- **Do** retain DM Sans and Manrope for the existing interface roles.
- **Do** keep visible keyboard focus and readable control targets.
- **Do** show preparation and recovery state beside the player.
- **Do** stack the workspace at narrow widths and preserve its search-to-play flow.

### Don't:

- **Don't** turn the player into a marketing landing page.
- **Don't** use green as the interface accent; the approved adaptation uses gold.
- **Don't** replace the native browser video controls with a custom player skin.
