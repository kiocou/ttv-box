---
name: Cinema Obsidian
colors:
  surface: '#111318'
  surface-dim: '#111318'
  surface-bright: '#37393e'
  surface-container-lowest: '#0c0e12'
  surface-container-low: '#1a1c20'
  surface-container: '#1e2024'
  surface-container-high: '#282a2e'
  surface-container-highest: '#333539'
  on-surface: '#e2e2e8'
  on-surface-variant: '#bdc8d1'
  inverse-surface: '#e2e2e8'
  inverse-on-surface: '#2f3035'
  outline: '#87929a'
  outline-variant: '#3e484f'
  surface-tint: '#7bd0ff'
  primary: '#8ed5ff'
  on-primary: '#00354a'
  primary-container: '#38bdf8'
  on-primary-container: '#004965'
  inverse-primary: '#00668a'
  secondary: '#ffb95f'
  on-secondary: '#472a00'
  secondary-container: '#ee9800'
  on-secondary-container: '#5b3800'
  tertiary: '#56e5a9'
  on-tertiary: '#003824'
  tertiary-container: '#30c88f'
  on-tertiary-container: '#004e34'
  error: '#ffb4ab'
  on-error: '#690005'
  error-container: '#93000a'
  on-error-container: '#ffdad6'
  primary-fixed: '#c4e7ff'
  primary-fixed-dim: '#7bd0ff'
  on-primary-fixed: '#001e2c'
  on-primary-fixed-variant: '#004c69'
  secondary-fixed: '#ffddb8'
  secondary-fixed-dim: '#ffb95f'
  on-secondary-fixed: '#2a1700'
  on-secondary-fixed-variant: '#653e00'
  tertiary-fixed: '#6ffbbe'
  tertiary-fixed-dim: '#4edea3'
  on-tertiary-fixed: '#002113'
  on-tertiary-fixed-variant: '#005236'
  background: '#111318'
  on-background: '#e2e2e8'
  surface-variant: '#333539'
  obsidian-base: '#0a0c10'
  tech-cyan: '#38bdf8'
  rating-gold: '#f59e0b'
  status-green: '#10b981'
  glass-surface: rgba(10, 12, 16, 0.85)
typography:
  display-lg:
    fontFamily: Plus Jakarta Sans
    fontSize: 48px
    fontWeight: '700'
    lineHeight: 56px
    letterSpacing: -0.02em
  headline-lg:
    fontFamily: Plus Jakarta Sans
    fontSize: 32px
    fontWeight: '700'
    lineHeight: 40px
    letterSpacing: -0.01em
  headline-md:
    fontFamily: Plus Jakarta Sans
    fontSize: 24px
    fontWeight: '600'
    lineHeight: 32px
  body-lg:
    fontFamily: Plus Jakarta Sans
    fontSize: 18px
    fontWeight: '400'
    lineHeight: 28px
  body-md:
    fontFamily: Plus Jakarta Sans
    fontSize: 16px
    fontWeight: '400'
    lineHeight: 24px
  metric-md:
    fontFamily: JetBrains Mono
    fontSize: 14px
    fontWeight: '500'
    lineHeight: 20px
    letterSpacing: 0.02em
  metric-sm:
    fontFamily: JetBrains Mono
    fontSize: 12px
    fontWeight: '500'
    lineHeight: 16px
    letterSpacing: 0.05em
  headline-lg-mobile:
    fontFamily: Plus Jakarta Sans
    fontSize: 28px
    fontWeight: '700'
    lineHeight: 36px
rounded:
  sm: 0.25rem
  DEFAULT: 0.5rem
  md: 0.75rem
  lg: 1rem
  xl: 1.5rem
  full: 9999px
spacing:
  unit: 4px
  gutter: 24px
  margin-desktop: 48px
  margin-mobile: 16px
  ratio-media: 2/3
---

## Brand & Style

This design system is a high-performance, immersive interface built for cinematic consumption and precision media management. It is characterized by an ultra-dark "lights-out" environment that minimizes visual noise, allowing content to take center stage. The target audience includes cinephiles, media collectors, and tech-savvy users who demand a premium, workstation-grade experience.

The design style is a fusion of **Glassmorphism** and **Minimalism**, utilizing deep translucency and technical accents to create a sense of depth and luxury. The emotional response is one of calm focus, reliability, and high-end technical sophistication.

**Key Principles:**
- **Cinematic Immersion:** High-contrast dark backgrounds ensure the UI recedes while imagery pops.
- **Precision Engineering:** Monospaced metrics and sharp technical accents suggest a high-performance system.
- **Dynamic Glow:** Interactive elements emit a "tech cyan" light, simulating the glow of high-end hardware in a dark room.

## Colors

The palette is anchored in **Obsidian Deep Black**, creating a void-like canvas that eliminates light bleed. **Tech Cyan** is the primary interactive signal, used for focus states, primary actions, and critical UI indicators. 

**Accent Gold** is reserved specifically for ratings and value-based metrics, providing a warm contrast to the cool obsidian base. **Status Green** is used for hardware health and security indicators. 

The color system relies on high-contrast relationships. Surfaces are defined by transparency and blur rather than lightness shifts, maintaining a consistent deep-black aesthetic across all layers.

## Typography

This system uses a dual-font approach. **Plus Jakarta Sans** provides a modern, approachable feel for the main UI and descriptive text, localized for Chinese (Simplified) characters to ensure readability. **JetBrains Mono** is used exclusively for metrics, technical metadata, and status readouts, evoking a high-precision instrument panel.

**Hierarchy Rules:**
- **Technical Metrics:** Use JetBrains Mono for durations, bitrates, and timestamps.
- **Titles:** Use bold weights for headlines to maintain authority against high-contrast backgrounds.
- **Localization:** Chinese characters should maintain a line-height at least 1.4x the font size to ensure legibility of complex strokes in dark mode.

## Layout & Spacing

The layout follows a **Fixed Grid** model on desktop (1440px max-width) to maintain cinematic proportions. It uses a 12-column structure with 24px gutters.

**Key Layout Rules:**
- **Media Cards:** Strictly adhere to a 2:3 golden ratio for movie posters.
- **Capsule Modules:** Navigation bars and functional modules (e.g., search, user profile) should be encapsulated in pill-shaped containers.
- **Breakpoints:** 
  - **Mobile (<768px):** 4 columns, 16px margins, fluid media cards.
  - **Desktop (>1024px):** 12 columns, 48px margins, fixed 2:3 media grid.

## Elevation & Depth

Hierarchy is established through **Glassmorphism** and **Dynamic Glows**. Physical shadows are replaced by light emission.

- **Level 0 (Base):** Solid #0a0c10 background.
- **Level 1 (Panels):** Capsule modules and sidebars using the Glass specification (85% opacity, 16px+ backdrop blur).
- **Interactive State:** Hovering over a card or button triggers a dynamic Cyan glow (`0 0 20px rgba(56, 189, 248, 0.3)`) and a subtle scale increase.
- **Separation:** Use 1px borders with 10% white opacity instead of shadows to define the edges of floating containers.

## Shapes

The shape language is defined by **Rounded** (0.5rem/8px) corners for primary content cards and structural elements. 

- **Capsule Elements:** Navigation bars, search inputs, and technical badges must use a full pill-shape (rounded-full) to signify high-level functional grouping.
- **Media Containers:** Use 8px (rounded-md) for posters to maintain a balance between a modern feel and technical precision.

## Components

### Buttons
- **Primary:** Capsule-shaped, Tech Cyan background with Obsidian text. On hover, apply a Cyan glow effect.
- **Metric Badges:** Small capsule-shaped, JetBrains Mono text, 1px subtle white border, no background.

### Media Cards
- **Poster Card:** 2:3 aspect ratio. On hover, the card scales 1.05x with a 1px Tech Cyan border and a diffused cyan outer glow.
- **Metadata Overlay:** Appears on the bottom 30% of the card using a glass gradient transition.

### Navigation
- **Capsule Nav:** A floating pill-shaped bar at the top or side. Uses 85% opacity obsidian glass with a 16px blur. Active states are indicated by a Tech Cyan dot indicator.

### Input Fields
- **Search:** Fully pill-shaped (capsule). 1px border (10% white). On focus, the border transitions to Tech Cyan with a soft glow.

### Status Indicators
- **System Health:** Small circular dots using Status Green.
- **Ratings:** Rating-Gold text paired with a star icon, always in JetBrains Mono.