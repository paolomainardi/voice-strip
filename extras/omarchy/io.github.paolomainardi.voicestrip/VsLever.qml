import QtQuick

// A bat-handle toggle switch: lever down is off, up is on.
Item {
  id: root

  property var pal
  property real k: 1
  property bool on: false

  signal toggled()

  Accessible.role: Accessible.CheckBox
  Accessible.checked: root.on

  Rectangle {
    anchors.fill: parent
    radius: 4 * root.k
    color: root.pal.switchBody
    border.width: 1 * root.k
    border.color: root.pal.buttonBorder

    Item {
      anchors.fill: parent
      rotation: root.on ? 180 : 0
      Behavior on rotation { NumberAnimation { duration: 180; easing.type: Easing.OutBack } }

      Rectangle {
        width: 6 * root.k
        height: parent.height * 0.36
        radius: width / 2
        color: root.pal.ink
        anchors.horizontalCenter: parent.horizontalCenter
        y: parent.height / 2
      }
    }
    Rectangle {
      anchors.centerIn: parent
      width: 12 * root.k
      height: width
      radius: width / 2
      color: root.pal.switchPivot
    }
  }

  MouseArea {
    anchors.fill: parent
    cursorShape: Qt.PointingHandCursor
    onClicked: root.toggled()
  }
}
