import QtQuick
import Quickshell
import Quickshell.Io
import qs.Commons
import qs.Commons as Commons
import qs.Ui

// Voice Strip on the Omarchy bar: a microphone with a small live output
// meter while processing, a crossed-out red microphone while muted, a
// half-dimmed one in bypass, a dimmed one while the app is closed. Left click opens the faceplate panel, middle
// click toggles processing, right click brings the window to the front.
//
// The widget keeps one connection to the app's control socket and sends
// `watch`, so the state and meters arrive as JSON lines without polling.
BarWidget {
  id: root
  moduleName: "io.github.paolomainardi.voicestrip"

  property var status: ({ running: false })
  property var presets: []
  property string fontsDir: ""

  readonly property string command: String(setting("command", "voice-strip") || "voice-strip")
  readonly property string look: String(setting("look", "Faceplate") || "Faceplate")
  readonly property bool showMeter: setting("showMeter", true) !== false
  readonly property bool hideWhenClosed: setting("hideWhenClosed", false) === true

  readonly property bool running: status.running === true
  readonly property bool on: running && status.on === true
  readonly property bool muted: running && status.muted === true
  readonly property real outVu: status.meters && status.meters.out_vu !== null && status.meters.out_vu !== undefined
    ? Number(status.meters.out_vu) : -60
  readonly property string socketPath: (Quickshell.env("XDG_RUNTIME_DIR") || "") + "/voice-strip/ctl.sock"

  readonly property bool opened: panelLoader.item ? panelLoader.item.opened === true : false
  readonly property bool popoutSwitchClosing: panelLoader.item
    ? panelLoader.item.popoutSwitchClosing === true
    : false

  function handleLine(line) {
    var s
    try { s = JSON.parse(String(line)) } catch (e) { return }
    if (!s || s.error) {
      if (s && s.error) console.warn("voice-strip: " + s.error)
      return
    }
    if (s.presets) root.presets = s.presets
    if (s.fonts_dir) root.fontsDir = s.fonts_dir
    root.status = s
  }

  // Sends a command on the open connection. Without one, `start` runs the
  // app in the background (no window) and `show` runs it with its window;
  // anything else waits for it to run.
  function send(line) {
    if (socket.connected) {
      socket.write(line + "\n")
      socket.flush()
    } else if (line === "start" || line === "show") {
      Quickshell.execDetached([root.command, "ctl", line])
      root.failures = 0
      root.launching = true
      reconnect.restart()
    }
  }

  // The knob moves many times a second; send at most every 50 ms.
  property int pendingIntensity: -1
  function setIntensity(v) {
    root.pendingIntensity = v
    if (!intensityThrottle.running) {
      root.flushIntensity()
      intensityThrottle.start()
    }
  }
  function flushIntensity() {
    if (root.pendingIntensity < 0) return
    root.send("intensity " + root.pendingIntensity)
    root.pendingIntensity = -1
  }
  Timer {
    id: intensityThrottle
    interval: 50
    onTriggered: root.flushIntensity()
  }

  function open() { if (panelLoader.item) panelLoader.item.open() }
  function close() { if (panelLoader.item) panelLoader.item.close() }
  function toggle() { if (panelLoader.item) panelLoader.item.toggle() }
  function closeForPopoutSwitch() { if (panelLoader.item) panelLoader.item.closeForPopoutSwitch() }

  function injectPanel() {
    if (!panelLoader.item) return
    panelLoader.item.bar = root.bar
    panelLoader.item.settings = root.settings
    panelLoader.item.anchorItem = button
    panelLoader.item.hostWidget = root
  }

  implicitWidth: button.implicitWidth
  implicitHeight: button.implicitHeight
  visible: !root.hideWhenClosed || root.running

  onBarChanged: injectPanel()
  onSettingsChanged: injectPanel()
  Component.onCompleted: {
    fontsProc.running = true
    probe.running = true
  }

  // The folder of the app's fonts, so the panel uses its type even while
  // the app is closed.
  Process {
    id: fontsProc
    command: [root.command, "--fonts-dir"]
    stdout: StdioCollector {
      waitForEnd: true
      onStreamFinished: {
        var dir = String(text || "").trim()
        if (dir !== "" && root.fontsDir === "") root.fontsDir = dir
      }
    }
  }

  Socket {
    id: socket
    path: root.socketPath
    parser: SplitParser {
      onRead: function(data) { root.handleLine(data) }
    }
    onConnectionStateChanged: {
      if (connected) {
        root.failures = 0
        root.launching = false
        write("watch\n")
        flush()
      } else {
        root.status = { running: false }
        reconnect.restart()
      }
    }
    onError: {
      root.failures++
      reconnect.restart()
    }
  }

  // While the app is closed, look for it every two seconds: the socket
  // must exist and answer `voice-strip ctl status` (a socket left behind
  // by a killed app does not). Probing this way keeps the journal quiet;
  // the widget connects only when the app is there.
  property int failures: 0
  property bool launching: false
  onLaunchingChanged: if (launching) launchTimeout.restart()
  Timer { id: launchTimeout; interval: 15000; onTriggered: root.launching = false }
  Timer {
    id: reconnect
    interval: root.launching ? 500 : (root.failures > 2 ? 10000 : 2000)
    onTriggered: if (!socket.connected && !probe.running) probe.running = true
  }
  Process {
    id: probe
    command: ["sh", "-c", "test -S \"$1\" && exec \"$2\" ctl status >/dev/null 2>&1", "sh",
      root.socketPath, root.command]
    onExited: function(code) {
      if (code === 0) {
        socket.connected = false
        socket.connected = true
      } else {
        reconnect.restart()
      }
    }
  }

  Loader {
    id: panelLoader
    active: true
    source: Qt.resolvedUrl("Panel.qml")
    visible: false
    onLoaded: {
      root.injectPanel()
      Qt.callLater(root.injectPanel)
    }
  }

  IpcHandler {
    target: root.moduleName
    function open(): void { root.open() }
    function close(): void { root.close() }
    function toggle(): void { root.toggle() }
    function show(): void { root.send("show") }
    function start(): void { root.send("start") }
    function power(): void { root.send("toggle") }
    function preset(id: string): void { root.send("preset " + id) }
    function noise(level: string): void { root.send("noise " + level) }
    function phones(): void { root.send("phones toggle") }
    function mute(): void { root.send("mute toggle") }
  }

  WidgetButton {
    id: button
    anchors.fill: parent
    bar: root.bar
    text: ""
    labelVisible: false
    hasVisualContent: true
    dimmed: !root.running
    fixedWidth: root.vertical ? -1 : content.implicitWidth + scaledHorizontalMargin * 2
    fixedHeight: root.vertical ? content.implicitHeight + scaledVerticalPadding * 2 : -1
    tooltipText: {
      if (!root.running) return "Voice Strip is not running\nClick for the panel and press START"
      var noise = root.status.noise === "off" ? "noise removal off" : "noise removal " + root.status.noise
      var mode = root.muted ? "Muted" : (root.on ? root.status.preset_label : "Bypass")
      return "Voice Strip · " + mode + " · " + noise
        + "\nMiddle click: on or off · Right click: window"
    }
    onPressed: function(buttonCode) {
      if (buttonCode === Qt.LeftButton) root.toggle()
      if (buttonCode === Qt.MiddleButton) root.send("toggle")
      if (buttonCode === Qt.RightButton) root.send("show")
    }
    onWheelMoved: function(delta) {
      if (!root.running) return
      var v = Math.max(0, Math.min(100, Number(root.status.intensity || 0) + (delta > 0 ? 5 : -5)))
      root.setIntensity(v)
    }

    Row {
      id: content
      anchors.centerIn: parent
      spacing: Style.spaceReal(5)
      readonly property color fg: root.bar ? root.bar.barForeground : Commons.Color.foreground

      Text {
        anchors.verticalCenter: parent.verticalCenter
        textFormat: Text.PlainText
        // Nerd Font glyphs: microphone, and microphone off while muted.
        text: root.muted ? "󰍭" : "󰍬"
        color: root.muted ? Commons.Color.urgent : content.fg
        opacity: root.running && !root.on && !root.muted ? 0.6 : 1
        font.family: root.bar ? root.bar.fontFamily : Style.font.family
        font.pixelSize: Style.font.body
        renderType: Text.NativeRendering
      }

      // Output level: five bars of rising height, like a signal meter.
      Row {
        id: meter
        visible: root.showMeter && root.on && !root.muted && !root.vertical
        anchors.verticalCenter: parent.verticalCenter
        spacing: Style.spaceReal(1.5)
        readonly property var steps: [-20, -12, -6, -2, 1]
        readonly property real full: Style.spaceReal(12)
        Repeater {
          model: 5
          Rectangle {
            required property int index
            readonly property bool lit: root.outVu >= meter.steps[index]
            anchors.bottom: parent.bottom
            width: Style.spaceReal(2.5)
            height: meter.full * (0.36 + 0.16 * index)
            radius: width / 2
            color: !lit ? content.fg
              : index === 4 ? Commons.Color.urgent
              : Commons.Color.accent
            opacity: lit ? 1 : 0.22
            Behavior on opacity { NumberAnimation { duration: 80 } }
          }
        }
      }
    }
  }
}
