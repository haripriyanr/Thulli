# Thulli UI System and Responsive Shell Design

## Goal
Create a coherent, reusable Slint UI system for Thulli that works across compact and wide desktop windows, supports light and dark themes, and respects Slint's Windows, macOS, Linux, Android, and iOS targets. Keep the existing Rust and Slint architecture.

## Design direction
- Light theme: warm off-white surfaces with muted teal accents, dark readable text, and soft warm-gray borders.
- Dark theme: charcoal surfaces, cool-white text, and the existing teal accent family.
- Shared tokens cover surfaces, text, accents, borders, spacing, radii, shadows, and interaction timing. Components consume tokens rather than choosing local colors and dimensions.
- Preserve the existing glossy magenta Thulli mark as brand artwork, with transparency.

## Navigation and shell
- Use the reference sidebar behavior: a narrow icon rail expands on hover to show labels, then collapses on pointer exit. Clicking navigates without pinning the rail open.
- The active destination has a clear filled/duotone icon and restrained tinted row indicator; passive destinations use crisp outlines and muted text.
- Keep explicit touch bounds. On touch platforms, tapping the rail toggles its expanded state since hover is unavailable.
- At narrow window widths, the shell reduces outer padding and hides nonessential labels before content becomes cramped. Wide layouts retain comfortable spacing.
- Keep window controls and theme control reachable and separated from the drag area.

## Background and motion
- Replace fixed-position oversized glows with responsive, low-contrast color fields that scale and reposition from the available window dimensions.
- Allow only slow ambient orb drift (16-second ease-in-out loop, low travel). No idle pulsing, shimmer, or floating decoration.
- Use short 120–180ms ease-out transitions for hover, active navigation, theme changes, and sidebar expansion. Respect reduced-motion preferences where Slint/platform support permits.
- Keep content contrast and readability above all background effects.

## Component library
Organize existing components into reusable families without creating one monolithic file:
- Foundations: theme tokens, spacing, typography, radii, elevation, breakpoint helpers.
- Navigation: responsive rail/sidebar, navigation row, divider, workspace mark.
- Actions: primary/secondary/quiet/danger buttons, icon button, segmented control.
- Inputs: text field, search, select, toggle, stepper, chip.
- Content: surface/card, task row, empty state, stat tile/panel, modal/sheet, toast/notice.
- Each component exposes only needed configurable properties and callbacks, defines explicit interactive hit areas, and avoids platform-specific assumptions in shared layout code.

## Responsive behavior
- Compact: prioritize one main content column, reduced gutters, condensed toolbar actions, and rail-first navigation.
- Medium: use two-column layouts where useful and keep labels visible when width allows.
- Wide: use full sidebar labels, multi-column boards, and larger content gutters.
- Use available parent width and visibility/layout conditions instead of relying on fixed global pixel coordinates. Establish minimum window constraints that remain usable on each target.

## Rollout
1. Establish tokens and theme application in the shell; fix the blank/empty content presentation so it is intentional and useful.
2. Implement and verify responsive navigation and shared shell components.
3. Refactor reusable buttons, fields, cards, and empty states to token-driven styling.
4. Migrate screens in batches, starting with Today, Tasks, Focus, and Notes, then remaining screens.
5. Validate at compact, medium, and wide dimensions and on Windows; keep other operating systems and mobile targets building as platform-specific work is added.

## Acceptance criteria
- Theme toggle visibly changes shell and shared components between warm light and charcoal dark palettes.
- Navigation expands on hover, collapses on exit, supports tap toggle, and communicates active state clearly.
- No shared reusable component depends on a hard-coded app-wide width where parent sizing can be used.
- Shell and key screens remain legible at minimum size and use available room on wide windows.
- Ambient background motion is restrained, responsive, and behind readable content.
- `cargo check`, `cargo test`, release build, and the required keyword audit pass after implementation.

## Risks and limits
- Several screens currently use fixed sizing and need staged migration; a token layer alone cannot make every screen responsive.
- Mobile targets need platform-specific packaging, permissions, focus/input behavior, and touch layouts. A desktop responsive layout is groundwork, not completed mobile delivery.
- Slint capabilities and reduced-motion signals differ by platform; keep the default motion minimal where a reliable system preference cannot be read.
