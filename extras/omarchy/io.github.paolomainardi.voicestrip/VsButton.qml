import QtQuick

// A faceplate push button with a pilot lamp, above or left of the label.
// The selected one looks pressed in: darker face, ink border, bolder label.
Item {
  id: root

  property var pal
  property real k: 1
  property string label: ""
  property string hint: ""
  property bool selected: false
  property bool lampTop: true
  property bool showLamp: true
  property color lampColor: pal ? pal.amber : "orange"
  property string fontFamily: ""
  property real fontSize: 11
  property real letterSpacing: 1

  signal clicked()

  Accessible.role: Accessible.Button
  Accessible.name: root.label
  Accessible.description: root.hint
  Accessible.checked: root.selected

  Rectangle {
    id: body
    anchors.fill: parent
    radius: 6 * root.k
    color: root.selected ? root.pal.buttonDown : (mouse.containsMouse ? root.pal.buttonHover : root.pal.button)
    border.width: (root.selected ? 1.5 : 1) * root.k
    border.color: root.selected ? root.pal.buttonDownBorder : root.pal.buttonBorder
    scale: mouse.pressed ? 0.97 : 1
    Behavior on color { ColorAnimation { duration: 120 } }
    Behavior on scale { NumberAnimation { duration: 90; easing.type: Easing.OutCubic } }

    // Top inner shadow on the pressed-in button.
    Rectangle {
      visible: root.selected
      anchors { left: parent.left; right: parent.right; top: parent.top; margins: parent.border.width }
      height: 3 * root.k
      radius: parent.radius
      color: "#000000"
      opacity: 0.08
    }

    Column {
      visible: root.lampTop
      anchors.centerIn: parent
      spacing: 6 * root.k
      VsLamp {
        visible: root.showLamp
        anchors.horizontalCenter: parent.horizontalCenter
        size: 10 * root.k
        lit: root.selected
        litColor: root.lampColor
        offColor: root.pal.lampOff
        rim: root.pal.lampRim
      }
      // Never wider than the button, even with a fallback font.
      Text {
        anchors.horizontalCenter: parent.horizontalCenter
        width: Math.min(implicitWidth, root.width - 2 * root.k)
        elide: Text.ElideRight
        text: root.label.toUpperCase()
        color: root.pal.ink
        font.family: root.fontFamily
        font.pixelSize: root.fontSize * root.k
        font.letterSpacing: root.letterSpacing * root.k
        font.weight: root.selected ? Font.DemiBold : Font.Normal
      }
    }

    Row {
      visible: !root.lampTop
      anchors.centerIn: parent
      spacing: 6 * root.k
      VsLamp {
        visible: root.showLamp
        anchors.verticalCenter: parent.verticalCenter
        size: 10 * root.k
        lit: root.selected
        litColor: root.lampColor
        offColor: root.pal.lampOff
        rim: root.pal.lampRim
      }
      Text {
        anchors.verticalCenter: parent.verticalCenter
        width: Math.min(implicitWidth, root.width - 18 * root.k)
        elide: Text.ElideRight
        text: root.label.toUpperCase()
        color: root.pal.ink
        font.family: root.fontFamily
        font.pixelSize: root.fontSize * root.k
        font.letterSpacing: root.letterSpacing * root.k
        font.weight: root.selected ? Font.DemiBold : Font.Normal
      }
    }
  }

  MouseArea {
    id: mouse
    anchors.fill: parent
    hoverEnabled: true
    cursorShape: Qt.PointingHandCursor
    onClicked: root.clicked()
  }
}
