import QtQuick

// Gain reduction LEDs: one per dB up to 12, yellow, then amber, then red.
Rectangle {
  id: root

  property var pal
  property real k: 1
  property real value: 0

  readonly property int lit: Math.round(Math.max(0, Math.min(12, value)))
  readonly property real pad: 5 * k
  readonly property real gap: 4 * k

  radius: 4 * k
  color: pal.ledWell

  Row {
    anchors.centerIn: parent
    spacing: root.gap
    Repeater {
      model: 12
      Rectangle {
        required property int index
        width: (root.width - root.pad * 2 - root.gap * 11) / 12
        height: root.height - root.pad * 2
        radius: 2 * root.k
        color: index >= root.lit ? root.pal.ledOff
          : index < 6 ? root.pal.ledLow
          : index < 9 ? root.pal.ledMid
          : root.pal.ledHigh
        Behavior on color { ColorAnimation { duration: 60 } }
      }
    }
  }
}
