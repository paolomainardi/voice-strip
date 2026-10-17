import QtQuick
import qs.Commons as Commons

// Colours of the panel. "Faceplate" and "Console" are the app's standard
// and advanced views (crates/ui-kit/src/theme.rs); "Omarchy" derives the
// same roles from the current theme.
QtObject {
  id: root

  property string look: "Faceplate"

  readonly property color themeBg: Commons.Color.popups.background
  readonly property color themeFg: Commons.Color.popups.text
  readonly property color themeAccent: Commons.Color.accent
  readonly property color themeUrgent: Commons.Color.urgent

  function mix(a, b, t) {
    var x = Qt.color(a)
    var y = Qt.color(b)
    return Qt.rgba(x.r + (y.r - x.r) * t, x.g + (y.g - x.g) * t, x.b + (y.b - x.b) * t, 1)
  }
  function pick(faceplate, console, omarchy) {
    if (root.look === "Console") return console
    if (root.look === "Omarchy") return omarchy
    return faceplate
  }

  // Body
  readonly property color frame: pick("#2A1F17", "#0E1013", mix(themeBg, "#000000", 0.35))
  readonly property color face: pick("#EAE2CE", "#161A1F", mix(themeBg, themeFg, 0.05))
  readonly property color screw: pick("#B7AC93", "#2A3038", mix(themeBg, themeFg, 0.22))
  readonly property color rule: pick("#C9BEA3", "#2A3038", mix(themeBg, themeFg, 0.16))
  readonly property color ink: pick("#2B2620", "#E9ECEF", themeFg)
  readonly property color inkSoft: pick("#5E5548", "#A0A8B3", mix(themeBg, themeFg, 0.62))
  readonly property color hover: pick("#8A3A22", "#F7C47A", mix(themeAccent, themeFg, 0.35))

  // Meters
  readonly property color vuFace: pick("#F3E7C4", "#0E1013", mix(themeBg, themeFg, 0.02))
  readonly property color vuBorder: pick("#2B2620", "#2A3038", mix(themeBg, themeFg, 0.25))
  readonly property color vuInk: pick("#2B2620", "#C9D1D9", themeFg)
  readonly property color needle: pick("#1E1A16", "#F0A23B", themeAccent)
  readonly property color pivot: pick("#2B2620", "#2A3038", mix(themeBg, themeFg, 0.25))
  readonly property color red: pick("#B4372A", "#D2553A", themeUrgent)
  readonly property color ledWell: pick("#2B2620", "#0E1013", mix(themeBg, "#000000", 0.4))
  readonly property color ledOff: pick("#4A4238", "#232830", mix(themeBg, themeFg, 0.12))
  readonly property color ledLow: pick("#E8C25A", "#E8C25A", mix(themeAccent, themeFg, 0.25))
  readonly property color ledMid: pick("#F0A23B", "#F0A23B", themeAccent)
  readonly property color ledHigh: pick("#D2553A", "#D2553A", themeUrgent)

  // Lamps and controls
  readonly property color amber: pick("#F0A23B", "#F0A23B", themeAccent)
  readonly property color phones: pick("#7FB3D9", "#6BB0F2", mix(themeAccent, "#6BB0F2", 0.5))
  readonly property color lampRim: pick("#2B2620", "#0E1013", mix(themeBg, "#000000", 0.3))
  readonly property color lampOff: pick("#6B5A3E", "#3A3F47", mix(themeBg, themeFg, 0.2))
  readonly property color button: pick("#D9CFB6", "#1D2229", mix(themeBg, themeFg, 0.08))
  readonly property color buttonHover: pick("#D2C7AC", "#232931", mix(themeBg, themeFg, 0.12))
  readonly property color buttonBorder: pick("#A89C80", "#2A3038", mix(themeBg, themeFg, 0.2))
  readonly property color buttonDown: pick("#C9BEA3", "#2A2418", mix(themeBg, themeAccent, 0.16))
  readonly property color buttonDownBorder: pick("#2B2620", "#F0A23B", themeAccent)
  readonly property color power: pick("#2B2620", "#0E1013", mix(themeBg, "#000000", 0.3))
  readonly property color powerText: pick("#EAE2CE", "#E9ECEF", themeFg)
  readonly property color knobOuter: pick("#2B2620", "#0E1013", mix(themeBg, "#000000", 0.3))
  readonly property color knobInner: pick("#3A332B", "#1D2229", mix(themeBg, themeFg, 0.08))
  readonly property color knobRing: pick("#4A4238", "#2A3038", mix(themeBg, themeFg, 0.2))
  readonly property color switchBody: pick("#C9BEA3", "#1D2229", mix(themeBg, themeFg, 0.1))
  readonly property color switchPivot: pick("#8C8270", "#7D8692", mix(themeBg, themeFg, 0.45))
}
