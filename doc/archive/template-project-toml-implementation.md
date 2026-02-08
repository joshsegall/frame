# Template-Based `project.toml` for `fr init`

## Goal

Replace the hardcoded `project.toml` generation in `fr init` with a
static template file embedded at compile time via `include_str!`. The
template is the single source of truth for default config values and
serves double duty as self-documenting reference in the repo.

## Reference Files

Read these before starting:

- `frame/concepts.md` — Configuration section (all `project.toml` keys
  and their defaults)
- `frame/cli.md` — `fr init` command spec
- The template file itself (location specified below)

## Template File

**Location:** `src/templates/project.toml`

The template is valid TOML with one placeholder: `{{PROJECT_NAME}}` in
the `[project]` section. All other values are real defaults. Sections
that are optional or user-customized (`[agent]`, `[[tracks]]`,
`[ids.prefixes]`, `[ui.tag_colors]`, `[ui.colors]`) are present as
commented-out examples with explanatory comments.

Sections with meaningful defaults (`[clean]`, `[ui]`) are uncommented
and active.

The template is provided separately — do not modify its content or
comments. Embed it as-is.

## Changes

### 1. Add the template to the source tree

Place the template at `src/templates/project.toml`. Create the
`src/templates/` directory if it doesn't exist.

### 2. Embed the template at compile time

In the init handler (likely `cli/handlers/init.rs` or wherever
`fr init` is implemented), replace any hardcoded `project.toml`
string construction with:

```rust
const PROJECT_TOML_TEMPLATE: &str = include_str!("../templates/project.toml");
```

Adjust the relative path based on the actual file location.

### 3. Implement template rendering

Create a function with this signature:

```rust
fn render_project_toml(
    name: &str,
    tracks: &[(String, String)],  // (track_id, display_name)
    existing_prefixes: &[String],
) -> String
```

**Step 1 — Name substitution:**

Replace `{{PROJECT_NAME}}` with the resolved project name.

```rust
let mut output = PROJECT_TOML_TEMPLATE.replace("{{PROJECT_NAME}}", name);
```

**Step 2 — Append tracks (if any):**

If `tracks` is non-empty, append `[[tracks]]` entries and an
`[ids.prefixes]` table to the end of the rendered string. Format:

```toml

[[tracks]]
id = "api"
name = "API Layer"
state = "active"
file = "tracks/api.md"

[[tracks]]
id = "ui"
name = "User Interface"
state = "active"
file = "tracks/ui.md"

[ids.prefixes]
api = "API"
ui = "UI"
```

Use the existing `generate_prefix` function for prefix derivation
(last segment, first 3 chars, uppercase, with collision resolution
against `existing_prefixes`).

**Step 3 — Return the string.** No TOML parsing or round-tripping.
The template is manipulated as raw text to preserve all comments and
formatting.

### 4. Remove hardcoded defaults

Delete any existing code that constructs the `project.toml` content
as a string literal or through a TOML builder. The template is now the
sole source. Ensure no default values for `[clean]`, `[ui]`, or other
sections are duplicated in Rust code — they live in the template only.

**Exception:** Runtime config parsing still needs default values via
`serde(default)` or equivalent for backwards compatibility with
existing projects that don't have every key. Those defaults must match
the template values. Add a comment on each default pointing to the
template as the source of truth:

```rust
/// Default: see src/templates/project.toml
#[serde(default = "default_done_threshold")]
pub done_threshold: usize,

fn default_done_threshold() -> usize { 250 }
```

### 5. Update `fr track new` (if applicable)

If `fr track new` currently generates or modifies `project.toml` by
reconstructing it from scratch rather than appending to the existing
file, update it to use an append strategy consistent with the init
handler. The template is only for new projects — existing projects
should never have their `project.toml` regenerated from template.

### 6. Tests

**Template embedding test:**
- Verify `PROJECT_TOML_TEMPLATE` is non-empty and contains
  `{{PROJECT_NAME}}`.

**Rendering test — no tracks:**
- Call `render_project_toml("My Project", &[], &[])`.
- Assert output contains `name = "My Project"`.
- Assert output does NOT contain `{{PROJECT_NAME}}`.
- Assert output does NOT contain `[[tracks]]` or `[ids.prefixes]`.
- Assert `[clean]` section is present with defaults.
- Assert `[ui]` section is present.

**Rendering test — with tracks:**
- Call with `tracks = [("api", "API Layer"), ("ui", "UI")]`.
- Assert output contains both `[[tracks]]` entries with correct
  fields.
- Assert `[ids.prefixes]` contains `api = "API"` and `ui = "UI"`.

**Rendering test — prefix collision:**
- Call with tracks that would collide (e.g., `"api"` and `"app"`).
- Assert prefixes are distinct.

**Round-trip validity test:**
- Parse the rendered output with `toml::from_str` to confirm it's
  valid TOML for all test cases above.

**Integration test:**
- Run `fr init --name "Test" --track api "API"` in a temp directory.
- Read the generated `project.toml`.
- Assert it contains expected sections, is valid TOML, and matches
  the template structure.

## Important Constraints

- **Do not parse and re-serialize the template through a TOML
  library.** This destroys comments. All manipulation is string-based.
- **The template file must remain valid TOML** (ignoring the
  `{{PROJECT_NAME}}` placeholder, which is inside a string value and
  thus valid TOML anyway).
- **Appended track/prefix sections** go at the end of the file, after
  all template content. Use a blank line separator.
