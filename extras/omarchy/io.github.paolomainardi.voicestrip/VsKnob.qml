import QtQuick

// The Intensity knob: a dark cap with an amber pointer over -135..135
// degrees and a ring of ticks. Drag up or right, or scroll, to turn it.
Item {
  id: root

  property var pal
  property real k: 1
  property int value: 60
  property bool interactive: true

  // While turning, the knob shows its own value; the app's answer takes
  // over again when the drag ends.
  property int dragValue: -1
  readonly property int shown: dragValue >= 0 ? dragValue : value
  readonly property real angle: -135 + 270 * shown / 100

  signal moved(int value)

  Accessible.role: Accessible.Dial
  Accessible.name: "Intensity"
  Accessible.description: shown + " percent"

  Repeater {
    model: 7
    Rectangle {
      required property int index
      readonly property real deg: -135 + index * 45
      width: 2 * root.k
      height: 7 * root.k
      radius: width / 2
      color: root.pal.ink
      x: root.width / 2 - width / 2
      y: 0
      transformOrigin: Item.Center
      transform: Rotation {
        origin.x: 1 * root.k
        origin.y: root.height / 2
        angle: deg
      }
    }
  }

  Rectangle {
    id: cap
    anchors.centerIn: parent
    width: root.width - 22 * root.k
    height: width
    radius: width / 2
    color: root.pal.knobOuter
    scale: mouse.pressed ? 0.985 : 1
    Behavior on scale { NumberAnimation { duration: 90 } }

    Rectangle {
      anchors.centerIn: parent
      width: parent.width - 10 * root.k
      height: width
      radius: width / 2
      color: root.pal.knobInner
      border.width: 1.5 * root.k
      border.color: root.pal.knobRing
    }

    Item {
      anchors.fill: parent
      rotation: root.angle
      Behavior on rotation {
        enabled: root.dragValue < 0
        NumberAnimation { duration: 140; easing.type: Easing.OutCubic }
      }
      Rectangle {
        width: 4 * root.k
        height: parent.height * 0.22
        radius: width / 2
        color: root.pal.amber
        anchors.horizontalCenter: parent.horizontalCenter
        y: parent.height * 0.1
      }
    }
  }

  MouseArea {
    id: mouse
    anchors.fill: parent
    enabled: root.interactive
    hoverEnabled: true
    cursorShape: pressed ? Qt.ClosedHandCursor : Qt.OpenHandCursor
    preventStealing: true
    property real startX: 0
    property real startY: 0
    property int startValue: 0
    onPressed: function(m) {
      startX = m.x
      startY = m.y
      startValue = root.value
      root.dragValue = root.value
    }
    onPositionChanged: function(m) {
      if (!pressed) return
      var delta = ((m.x - startX) - (m.y - startY)) / (1.6 * root.k)
      var v = Math.max(0, Math.min(100, Math.round(startValue + delta)))
      if (v !== root.dragValue) {
        root.dragValue = v
        root.moved(v)
      }
    }
    onReleased: release.restart()
    onCanceled: release.restart()
    onWheel: function(w) {
      var step = w.angleDelta.y > 0 ? 2 : (w.angleDelta.y < 0 ? -2 : 0)
      var v = Math.max(0, Math.min(100, root.shown + step))
      if (v !== root.shown) {
        root.dragValue = v
        root.moved(v)
        release.restart()
      }
    }
  }

  // Keep showing the dragged value until the app has had time to answer.
  Timer { id: release; interval: 250; onTriggered: root.dragValue = -1 }
}
