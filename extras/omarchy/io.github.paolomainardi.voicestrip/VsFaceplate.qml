import QtQuick

// A compact Voice Strip faceplate: the app's standard view, sized for a
// bar panel. It reads the state from `host` (the bar widget) and sends
// commands back through `host.send()`.
Item {
  id: root

  property var host: null
  property var pal
  property real k: 1
  // Whether the panel is open; the needles stop moving while it is not.
  property bool active: true
  property string displayFont: ""
  property string monoFont: ""

  readonly property var st: host ? host.status : ({ running: false })
  readonly property bool running: st.running === true
  readonly property bool on: running && st.on === true
  readonly property var meters: st.meters || null
  readonly property var presets: host && host.presets.length > 0 ? host.presets : [
    { id: "broadcast", label: "Broadcast", description: "" },
    { id: "podcast", label: "Podcast", description: "" },
    { id: "call", label: "Call", description: "" },
    { id: "stream", label: "Stream", description: "" },
    { id: "narration", label: "Narration", description: "" }
  ]
  readonly property var noiseLevels: ["off", "light", "medium", "strong"]

  // Preset hovered with the pointer, to preview its description.
  property string hoverPreset: ""

  function send(line) { if (host) host.send(line) }
  // A VU input from the meters; JSON null (silence) is minus infinity.
  function level(key) {
    var v = meters ? meters[key] : null
    return v === null || v === undefined ? -Infinity : Number(v)
  }
  function num(v, digits) { return (v === null || v === undefined || !isFinite(v)) ? null : Number(v).toFixed(digits) }
  function descriptionFor(id) {
    for (var i = 0; i < presets.length; i++) if (presets[i].id === id) return presets[i].description
    return ""
  }

  readonly property real contentWidth: 320 * k
  implicitWidth: 372 * k
  implicitHeight: frame.implicitHeight

  component Label: Text {
    color: root.pal.ink
    font.family: root.displayFont
    font.pixelSize: 13 * root.k
    font.letterSpacing: 1.6 * root.k
    font.weight: Font.Normal
  }
  component Readout: Text {
    color: root.pal.inkSoft
    font.family: root.monoFont
    font.pixelSize: 11 * root.k
  }
  component Link: Text {
    id: link
    signal activated()
    color: linkMouse.containsMouse ? root.pal.hover : root.pal.ink
    font.family: root.displayFont
    font.pixelSize: 11.5 * root.k
    font.letterSpacing: 2.4 * root.k
    font.weight: Font.DemiBold
    Accessible.role: Accessible.Link
    Accessible.name: text
    Behavior on color { ColorAnimation { duration: 120 } }
    MouseArea {
      id: linkMouse
      anchors.fill: parent
      anchors.margins: -4 * root.k
      hoverEnabled: true
      cursorShape: Qt.PointingHandCursor
      onClicked: link.activated()
    }
  }

  Rectangle {
    id: frame
    width: root.width
    implicitHeight: face.implicitHeight + 16 * root.k
    height: implicitHeight
    radius: 14 * root.k
    color: root.pal.frame

    Rectangle {
      id: face
      x: 8 * root.k
      y: 8 * root.k
      width: parent.width - 16 * root.k
      implicitHeight: body.implicitHeight + 36 * root.k
      height: implicitHeight
      radius: 9 * root.k
      color: root.pal.face

      Repeater {
        model: [[0, 0], [1, 0], [0, 1], [1, 1]]
        Rectangle {
          required property var modelData
          width: 6 * root.k
          height: width
          radius: width / 2
          color: root.pal.screw
          x: modelData[0] ? face.width - 9 * root.k - width : 9 * root.k
          y: modelData[1] ? face.height - 9 * root.k - width : 9 * root.k
        }
      }

      Column {
        id: body
        x: 18 * root.k
        y: 20 * root.k
        width: root.contentWidth
        spacing: 0

        // Title and power button.
        Item {
          width: parent.width
          height: 44 * root.k
          Column {
            anchors.verticalCenter: parent.verticalCenter
            spacing: 2 * root.k
            Text {
              text: "VOICE STRIP"
              color: root.pal.ink
              font.family: root.displayFont
              font.pixelSize: 21 * root.k
              font.letterSpacing: 2.8 * root.k
              font.weight: Font.DemiBold
            }
            Text {
              text: "VOICE PROCESSOR"
              color: root.pal.inkSoft
              font.family: root.displayFont
              font.pixelSize: 9.5 * root.k
              font.letterSpacing: 2.2 * root.k
            }
          }
          // MUTE: faceplate-coloured, red with a lit lamp while muted.
          Rectangle {
            id: mute
            visible: root.running
            anchors.right: power.left
            anchors.rightMargin: 8 * root.k
            anchors.verticalCenter: parent.verticalCenter
            width: muteRow.implicitWidth + 18 * root.k
            height: 32 * root.k
            radius: 6 * root.k
            readonly property bool muted: root.st.muted === true
            color: muted ? root.pal.red : (muteMouse.containsMouse ? root.pal.buttonHover : root.pal.button)
            border.width: 1 * root.k
            border.color: muted ? root.pal.red : (muteMouse.containsMouse ? root.pal.ink : root.pal.buttonBorder)
            scale: muteMouse.pressed ? 0.96 : 1
            Behavior on color { ColorAnimation { duration: 120 } }
            Behavior on scale { NumberAnimation { duration: 90 } }
            Accessible.role: Accessible.Button
            Accessible.name: muted ? "Muted" : "Mute"
            Row {
              id: muteRow
              anchors.centerIn: parent
              spacing: 6 * root.k
              VsLamp {
                anchors.verticalCenter: parent.verticalCenter
                size: 9 * root.k
                lit: mute.muted
                litColor: "#FF6B57"
                offColor: root.pal.lampOff
                rim: mute.muted ? root.pal.red : root.pal.lampRim
              }
              Text {
                anchors.verticalCenter: parent.verticalCenter
                text: mute.muted ? "MUTED" : "MUTE"
                color: mute.muted ? root.pal.face : root.pal.ink
                font.family: root.displayFont
                font.pixelSize: 12 * root.k
                font.letterSpacing: 1.5 * root.k
                font.weight: mute.muted ? Font.DemiBold : Font.Normal
              }
            }
            MouseArea {
              id: muteMouse
              anchors.fill: parent
              hoverEnabled: true
              cursorShape: Qt.PointingHandCursor
              onClicked: root.send("mute toggle")
            }
          }
          Rectangle {
            id: power
            anchors.right: parent.right
            anchors.verticalCenter: parent.verticalCenter
            width: powerRow.implicitWidth + 20 * root.k
            height: 32 * root.k
            radius: 6 * root.k
            color: root.pal.power
            scale: powerMouse.pressed ? 0.96 : 1
            Behavior on scale { NumberAnimation { duration: 90 } }
            Accessible.role: Accessible.Button
            Accessible.name: !root.running ? "Start Voice Strip" : (root.on ? "Processing on" : "Bypass")
            Row {
              id: powerRow
              anchors.centerIn: parent
              spacing: 7 * root.k
              VsLamp {
                anchors.verticalCenter: parent.verticalCenter
                size: 9 * root.k
                lit: root.on
                litColor: root.pal.amber
                offColor: root.pal.lampOff
                rim: root.pal.power
              }
              Text {
                anchors.verticalCenter: parent.verticalCenter
                text: !root.running ? "START" : (root.on ? "ON" : "BYPASS")
                color: root.pal.powerText
                font.family: root.displayFont
                font.pixelSize: 12 * root.k
                font.letterSpacing: 1.5 * root.k
                opacity: powerMouse.containsMouse ? 1 : 0.92
              }
            }
            MouseArea {
              id: powerMouse
              anchors.fill: parent
              hoverEnabled: true
              cursorShape: Qt.PointingHandCursor
              onClicked: root.running ? root.send("toggle") : root.send("start")
            }
          }
        }

        Item { width: 1; height: 14 * root.k }

        // Live part: everything below dims while the app is closed.
        Column {
          width: parent.width
          spacing: 0
          enabled: root.running
          opacity: root.running ? 1 : 0.55
          Behavior on opacity { NumberAnimation { duration: 200 } }

          Row {
            spacing: 12 * root.k
            VsVuMeter {
              width: (root.contentWidth - 12 * root.k) / 2
              height: width * 0.65
              pal: root.pal
              fontFamily: root.displayFont
              live: root.running
              active: root.active
              level: root.level("in_level")
            }
            VsVuMeter {
              width: (root.contentWidth - 12 * root.k) / 2
              height: width * 0.65
              pal: root.pal
              fontFamily: root.displayFont
              live: root.running
              active: root.active
              level: root.level("out_level")
            }
          }

          Item { width: 1; height: 7 * root.k }

          Row {
            spacing: 12 * root.k
            Item {
              width: (root.contentWidth - 12 * root.k) / 2
              height: 18 * root.k
              Label { text: "INPUT"; anchors.verticalCenter: parent.verticalCenter }
              VsLamp {
                anchors.right: parent.right
                anchors.verticalCenter: parent.verticalCenter
                size: 10 * root.k
                lit: root.meters ? root.meters.peak === true : false
                litColor: root.pal.ledHigh
                offColor: root.pal.lampOff
                rim: root.pal.lampRim
              }
            }
            Item {
              width: (root.contentWidth - 12 * root.k) / 2
              height: 18 * root.k
              Label { text: "OUTPUT"; anchors.verticalCenter: parent.verticalCenter }
              Readout {
                anchors.right: parent.right
                anchors.verticalCenter: parent.verticalCenter
                text: {
                  var v = root.meters ? root.num(root.meters.lufs, 1) : null
                  return (v === null ? "--.-" : v) + " LUFS"
                }
              }
            }
          }

          Item { width: 1; height: 12 * root.k }

          Item {
            width: parent.width
            height: 18 * root.k
            Label { id: grLabel; text: "GR"; anchors.verticalCenter: parent.verticalCenter }
            VsLedBar {
              anchors.left: parent.left
              anchors.leftMargin: 34 * root.k
              anchors.right: grText.left
              anchors.rightMargin: 10 * root.k
              anchors.verticalCenter: parent.verticalCenter
              height: 18 * root.k
              pal: root.pal
              k: root.k
              value: root.meters ? Number(root.meters.gr || 0) : 0
            }
            Readout {
              id: grText
              anchors.right: parent.right
              anchors.verticalCenter: parent.verticalCenter
              horizontalAlignment: Text.AlignRight
              width: 56 * root.k
              text: {
                var g = root.meters ? Number(root.meters.gr || 0) : 0
                return (g > 0.05 ? "-" + g.toFixed(1) : "0.0") + " dB"
              }
            }
          }

          Item { width: 1; height: 14 * root.k }

          Row {
            spacing: 6 * root.k
            Repeater {
              model: root.presets
              VsButton {
                required property var modelData
                required property int index
                width: (root.contentWidth - 24 * root.k) / 5
                height: 48 * root.k
                pal: root.pal
                k: root.k
                label: modelData.label
                hint: modelData.description + " (key " + (index + 1) + ")"
                selected: root.st.preset === modelData.id
                fontFamily: root.displayFont
                fontSize: 10.5
                letterSpacing: 0.7
                onClicked: root.send("preset " + modelData.id)
                HoverHandler {
                  onHoveredChanged: {
                    if (hovered) root.hoverPreset = modelData.id
                    else if (root.hoverPreset === modelData.id) root.hoverPreset = ""
                  }
                }
              }
            }
          }

          Item { width: 1; height: 8 * root.k }

          Readout {
            width: parent.width
            horizontalAlignment: Text.AlignHCenter
            elide: Text.ElideRight
            font.pixelSize: 9.5 * root.k
            text: {
              if (!root.running) return "Voice Strip is not running"
              if (root.hoverPreset !== "") return root.descriptionFor(root.hoverPreset)
              return root.st.preset_description || ""
            }
          }

          Item { width: 1; height: 14 * root.k }

          Item {
            width: parent.width
            height: 32 * root.k
            Label { text: "NOISE"; anchors.verticalCenter: parent.verticalCenter }
            Row {
              anchors.right: parent.right
              spacing: 6 * root.k
              Repeater {
                model: root.noiseLevels
                VsButton {
                  required property var modelData
                  width: (root.contentWidth - 64 * root.k - 18 * root.k) / 4
                  height: 32 * root.k
                  pal: root.pal
                  k: root.k
                  lampTop: false
                  label: modelData
                  hint: "Noise removal " + modelData
                  selected: root.st.noise === modelData
                  fontFamily: root.displayFont
                  fontSize: 11
                  letterSpacing: 0.8
                  onClicked: root.send("noise " + modelData)
                }
              }
            }
          }

          Item { width: 1; height: 18 * root.k }

          // Phones, Intensity, latency.
          Item {
            width: parent.width
            height: 118 * root.k

            Column {
              width: 70 * root.k
              anchors.verticalCenter: knob.verticalCenter
              spacing: 8 * root.k
              VsLever {
                anchors.horizontalCenter: parent.horizontalCenter
                width: 34 * root.k
                height: 42 * root.k
                pal: root.pal
                k: root.k
                on: root.st.phones === true
                Accessible.name: "Headphones"
                onToggled: root.send("phones toggle")
              }
              Label {
                anchors.horizontalCenter: parent.horizontalCenter
                text: "PHONES"
                font.pixelSize: 11.5 * root.k
              }
              VsLamp {
                anchors.horizontalCenter: parent.horizontalCenter
                size: 10 * root.k
                lit: root.st.phones === true
                litColor: root.pal.phones
                offColor: root.pal.lampOff
                rim: root.pal.lampRim
              }
            }

            VsKnob {
              id: knob
              anchors.horizontalCenter: parent.horizontalCenter
              y: 0
              width: 96 * root.k
              height: width
              pal: root.pal
              k: root.k
              value: Number(root.st.intensity || 0)
              onMoved: function(v) { root.host && root.host.setIntensity(v) }
            }
            Row {
              anchors.horizontalCenter: knob.horizontalCenter
              anchors.top: knob.bottom
              anchors.topMargin: 2 * root.k
              spacing: 6 * root.k
              Label { id: intensityLabel; text: "INTENSITY"; font.pixelSize: 12 * root.k }
              Readout {
                anchors.baseline: intensityLabel.baseline
                text: root.running ? knob.shown : "--"
                color: root.pal.ink
                font.pixelSize: 12 * root.k
              }
            }

            Column {
              width: 70 * root.k
              anchors.right: parent.right
              anchors.verticalCenter: knob.verticalCenter
              spacing: 3 * root.k
              Label {
                anchors.horizontalCenter: parent.horizontalCenter
                text: "MIC"
                font.pixelSize: 11.5 * root.k
              }
              Readout {
                anchors.horizontalCenter: parent.horizontalCenter
                text: root.num(root.st.mic_latency_ms, 1) || "--.-"
                color: root.pal.ink
                font.pixelSize: 17 * root.k
              }
              Readout {
                anchors.horizontalCenter: parent.horizontalCenter
                text: "ms"
                font.pixelSize: 10 * root.k
              }
            }
          }
        }

        Item { width: 1; height: 12 * root.k }
        Rectangle { width: parent.width; height: Math.max(1, root.k); color: root.pal.rule }
        Item { width: 1; height: 12 * root.k }

        Item {
          width: parent.width
          height: 18 * root.k
          Readout {
            anchors.left: parent.left
            anchors.right: links.left
            anchors.rightMargin: 10 * root.k
            anchors.verticalCenter: parent.verticalCenter
            elide: Text.ElideRight
            font.pixelSize: 10.5 * root.k
            color: root.st.notice ? root.pal.red : root.pal.inkSoft
            text: {
              if (!root.running) return "START runs it in the background"
              if (root.st.notice) return root.st.notice
              if (root.st.calibrating) return "Calibrating..."
              return root.st.input || ""
            }
          }
          Row {
            id: links
            anchors.right: parent.right
            anchors.verticalCenter: parent.verticalCenter
            spacing: 18 * root.k
            Link {
              text: "WINDOW"
              onActivated: { root.send("show"); if (root.host) root.host.close() }
            }
            Link {
              visible: root.running
              text: "QUIT"
              onActivated: root.send("quit")
            }
          }
        }
      }
    }
  }
}
