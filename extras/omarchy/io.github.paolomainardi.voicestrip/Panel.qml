import QtQuick
import Quickshell
import qs.Commons
import qs.Commons as Commons
import qs.Ui

// The panel under the bar icon: a compact Voice Strip faceplate with live
// VU meters, gain reduction, presets, noise removal, headphones and the
// Intensity knob.
//
// Keys: Space or Enter processing on or off, M mute, 1 to 5 presets, N next
// noise level, Left and Right (or H and L) Intensity, P headphones,
// W window, Esc close.
Panel {
  id: root
  moduleName: "io.github.paolomainardi.voicestrip"
  manageIpc: false

  property var anchorItem: null
  property var hostWidget: null

  readonly property var host: root.hostWidget
  readonly property real uiScale: Math.max(0.8, Style.spaceReal(1))
  readonly property string barFont: root.bar ? root.bar.fontFamily : Style.font.family
  // The app's fonts, from the folder it reports.
  readonly property string fontsUrl: host && host.fontsDir !== "" ? "file://" + host.fontsDir + "/" : ""

  function open() { root.controller.show() }
  function close() { root.controller.hide() }
  function toggle() { root.opened ? close() : open() }
  function switchPanel(direction) {
    if (root.bar && typeof root.bar.switchPanelFrom === "function")
      return root.bar.switchPanelFrom(root.hostWidget || root, direction)
    return false
  }

  function send(line) { if (host) host.send(line) }
  function key(text) {
    if (!host || !host.running) {
      if (text === "w") root.send("show")
      return
    }
    var st = host.status
    var n = parseInt(text)
    if (n >= 1 && n <= host.presets.length) {
      root.send("preset " + host.presets[n - 1].id)
    } else if (text === "n") {
      var levels = ["off", "light", "medium", "strong"]
      root.send("noise " + levels[(levels.indexOf(st.noise) + 1) % levels.length])
    } else if (text === "m") {
      root.send("mute toggle")
    } else if (text === "p") {
      root.send("phones toggle")
    } else if (text === "w") {
      root.send("show")
      root.close()
    }
  }
  function nudge(step) {
    if (!host || !host.running) return
    host.setIntensity(Math.max(0, Math.min(100, Number(host.status.intensity || 0) + step)))
  }

  // The app's own type, from the folder it reports; the bar font until then.
  FontLoader {
    id: oswald
    source: root.fontsUrl !== "" ? root.fontsUrl + "Oswald-Regular.ttf" : ""
  }
  // Same family, semibold face: titles and selected buttons.
  FontLoader {
    source: root.fontsUrl !== "" ? root.fontsUrl + "Oswald-SemiBold.ttf" : ""
  }
  FontLoader {
    id: plexMono
    source: root.fontsUrl !== "" ? root.fontsUrl + "IBMPlexMono-Regular.ttf" : ""
  }

  VsPalette {
    id: palette
    look: host ? host.look : "Faceplate"
  }

  KeyboardPanel {
    id: panel
    anchorItem: root.anchorItem
    owner: root.hostWidget || root
    bar: root.bar
    open: root.opened
    focusTarget: keyCatcher
    padding: Style.space(8)
    // The card's width includes its padding and border; its height helper
    // adds them already.
    contentWidth: panel.fittedContentWidth(372 * root.uiScale + panel.padding * 2
      + Border.left(panel.borderSpec) + Border.right(panel.borderSpec))
    contentHeight: panel.fittedContentHeight(faceplate.implicitHeight)

    PanelKeyCatcher {
      id: keyCatcher
      anchors.fill: parent
      onCloseRequested: root.close()
      onTabRequested: function(direction) { root.switchPanel(direction) }
      onActivateRequested: root.host && root.host.running ? root.send("toggle") : root.send("start")
      onMoveRequested: function(dx, dy) { root.nudge(dx * 5 - dy * 5) }
      onTextKey: function(t) { root.key(String(t).toLowerCase()) }

      // Drawn for the width the card gives it, so it never spills out.
      VsFaceplate {
        id: faceplate
        width: keyCatcher.width
        host: root.host
        active: root.opened
        pal: palette
        k: keyCatcher.width > 0 ? keyCatcher.width / 372 : root.uiScale
        displayFont: oswald.status === FontLoader.Ready ? oswald.name : root.barFont
        monoFont: plexMono.status === FontLoader.Ready ? plexMono.name : root.barFont
      }
    }
  }
}
