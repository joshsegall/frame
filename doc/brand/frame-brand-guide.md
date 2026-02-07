# Frame — Brand Guide

## Identity

**Name:** Frame
**Command:** `fr`
**Tagline:** your backlog is plain text
**One-liner:** A markdown task tracker for the terminal.

Frame is a file-based task tracker with a terminal UI. Tasks live in
markdown files inside your repo. The TUI is for humans. The CLI is for
agents. Git is your history.

---

## Logo

The Frame logo is two open rounded brackets framing a right-pointing
play triangle. It directly references the `[>]` active-state checkbox
that users see throughout the TUI — Frame's visual signature.

The brackets are the structure. The triangle is the momentum.

### Construction

The mark sits on a 200×200 unit grid. The brackets are 10-unit stroked
paths with rounded endcaps and a corner radius of 30 units. The play
triangle is a filled path centered vertically, offset slightly right
of center to feel balanced within the brackets.

### Variants

| File                     | Usage                                      |
|--------------------------|--------------------------------------------|
| `frame-icon.svg`         | Primary icon, transparent background       |
| `frame-icon-dark-bg.svg` | App icon, favicon (dark rounded-rect bg)   |
| `frame-icon-light.svg`   | For use on light backgrounds               |
| `frame-icon-mono.svg`    | Single-color (`currentColor`), embeds, etc.|
| `frame-wordmark.svg`     | Horizontal lockup: icon + "frame"          |
| `frame-wordmark-tagline.svg` | Lockup with tagline                    |

### Minimum Size

The icon remains legible down to 14×14px. At sizes below 28px, the
stroke weight should be increased proportionally (the SVGs handle this
via the viewBox scaling). For pixel-perfect rendering at very small
sizes (favicons), export a dedicated 16×16 or 32×32 PNG with adjusted
stroke weights.

### Clear Space

Maintain a minimum clear space of 25% of the icon's width on all sides.
For the wordmark lockup, maintain at least 50% of the icon's width on
all sides.

### Don'ts

- Don't rotate or skew the logo
- Don't change the proportions of brackets to triangle
- Don't add drop shadows, gradients, or effects
- Don't place the full-color version on busy or mid-tone backgrounds
- Don't substitute the play triangle with other shapes
- Don't outline the triangle — it's always a solid fill

---

## Color Palette

### Primary

| Name        | Hex       | Usage                                      |
|-------------|-----------|--------------------------------------------|
| Background  | `#0C001B` | App background, dark surfaces              |
| Text        | `#A09BFE` | Bracket color, IDs, metadata, structure    |
| Text Bright | `#FFFFFF` | Task titles, selected/active text          |
| Highlight   | `#FB4196` | Play triangle, active state, mode labels   |
| Dim         | `#5A5580` | Done items, tree lines, deemphasized text  |

### Accent

| Name   | Hex       | Usage                          |
|--------|-----------|--------------------------------|
| Red    | `#FF4444` | Blocked state, bug tag         |
| Yellow | `#FFD700` | Parked state, needs-input tag  |
| Green  | `#44FF88` | Ready tag                      |
| Cyan   | `#44DDFF` | Design tag, spec refs          |
| Purple | `#CC66FF` | CC/cc-added tags               |
| Blue   | `#4488FF` | Research tag                   |

### Extended

| Name            | Hex       | Usage                              |
|-----------------|-----------|------------------------------------|
| Light Purple    | `#C0BBFF` | Wordmark text, secondary text      |
| Dark Purple     | `#6B65B0` | Bracket color on light backgrounds |
| Dark Text       | `#4A4580` | Wordmark on light backgrounds      |

### Background Adaptation

On dark backgrounds (default): brackets `#A09BFE`, triangle `#FB4196`.
On light backgrounds: brackets `#6B65B0`, triangle `#FB4196` (unchanged).
The pink triangle is the constant across all contexts.

---

## Typography

### Wordmark

**JetBrains Mono**, weight 500 (Medium), lowercase.

The wordmark is always lowercase `frame` to match the `fr` command and
the terminal-native identity. Color: `#C0BBFF` on dark, `#4A4580` on
light.

### Tagline

**JetBrains Mono**, weight 400 (Regular), lowercase.
Color: `#5A5580` (dim).

### Application UI

The TUI uses the terminal's configured font. Documentation and marketing
materials use JetBrains Mono for code/technical content and a clean sans
serif (DM Sans or similar) for body text.

---

## ASCII / Unicode Representation

For plain-text contexts (README headers, terminal output, CLI help),
the logo is rendered in pure ASCII:

```
[>] frame
```

or with the tagline:

```
[>] frame — your backlog is plain text
```

Pure ASCII `[>]` is the canonical text representation. This is
intentional — it matches the actual markdown checkbox syntax, renders
identically in every terminal and font, and avoids Unicode rendering
inconsistencies. Do not substitute with `▸` or other Unicode triangles.

---

## Voice & Tone

Frame's voice is:

- **Direct.** Say what it does. No marketing fluff.
- **Technical but warm.** Speaks developer, not enterprise.
- **Opinionated.** Frame has clear opinions about how task tracking
  should work and isn't shy about them.
- **Concise.** Fewer words. Let the tool speak for itself.

Examples:

> ✓ "Tasks are markdown files in your repo. Position is priority.
>    Git is your history."
>
> ✗ "Frame revolutionizes your workflow with an innovative approach
>    to task management that leverages the power of plaintext."

> ✓ "your backlog is plain text"
>
> ✗ "Simplify your project management experience"

---

## Usage Examples

### README header

```markdown
# [>] frame

**your backlog is plain text**

A markdown task tracker with a terminal UI for humans and a CLI for agents.
```

### CLI help banner

```
[>] frame v0.1.0
A markdown task tracker for the terminal

Usage: fr <command> [options]
```
