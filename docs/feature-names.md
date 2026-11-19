# Feature naming

Nephclick PhotoLab names its own raw-processing workspace **RAW Develop**, the adjustment **RAW Develop Filter**, and its raw preferences **RAW Defaults**. All 34 shipped non-English catalogs use the new keys and localized labels.

## Compatibility boundary

The `filter.cameraRaw` command, raw preference IDs, Rust type names and serialized settings remain stable. Existing documents retain their own saved filter names. New filters get the neutral display name.

Adobe Camera Raw PSD/action descriptor class names, XMP namespaces and the settings-file compatibility descriptions are unchanged. Import/export mapping code and attribution are unchanged.

A Photoshop action's step name is stored in the action file, so a saved Camera Raw step keeps Photoshop's "Camera Raw Filter" wording and a loaded action round-trips unchanged. Only Nephclick PhotoLab's own step labels, menus and windows say RAW Develop Filter.

## Validation before readiness

Review menu and dialog layout in the supported interfaces, run the engine/UI tests and the existing PSD/action/XMP round-trip tests, and complete the normal platform gates. This is a feature naming change, not a legal determination.

The catalog cleanup removes 136 dormant rows for branded composer labels, PDF presets, old Photoshop defaults and the unused raw-opening label. No active non-localization call sites use those keys. Current Nephclick PhotoLab defaults and required compatibility descriptions remain intact. Regression guards cover production Rust display literals and catalog keys; explicit exceptions document interchange descriptions and identifiers.

## AI features (2026-11-19)

Nephclick PhotoLab's own names for its AI features. Each is a plain description of what it does; none is another
product's feature name. The reference application calls comparable features by its own trademarked names, which
are not used anywhere in the interface.

| Name | What it is | Command / place |
|---|---|---|
| **AI Fill** | Paints a prompt into the selected area and adds it as a new layer masked to the selection | Selection Bar; `layer.new.fromImage` places the result |
| **Selection Bar** | The contextual task bar under a selection (AI Fill, Invert, Mask, Deselect) | `tools/selection_bar.rs`; Preferences › AI Models › Show the Selection Bar |
| **Color Palettes** | Panel of five colours, five gradients and five patterns, from a prompt or themed sets | Window › Color Palettes (`window.panel.colorPalettes`) |
| **AI Models** | Preferences page for the AI service, models and API key | Edit › Preferences › AI Models… (`edit.preferences.ai`) |
| **Layer from Image** | A new layer from image bytes over a rectangle | `layer.new.fromImage` (no menu item) |
