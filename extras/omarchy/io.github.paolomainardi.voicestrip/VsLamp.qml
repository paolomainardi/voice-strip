import QtQuick

// A pilot lamp: a domed disc with a rim, and a soft halo when lit.
Item {
  id: root

  property real size: 10
  property bool lit: false
  property color litColor: "#F0A23B"
  property color offColor: "#6B5A3E"
  property color rim: "#2B2620"

  implicitWidth: size
  implicitHeight: size

  Rectangle {
    anchors.centerIn: parent
    width: root.size * 2.6
    height: width
    radius: width / 2
    color: root.litColor
    opacity: root.lit ? 0.16 : 0
    Behavior on opacity { NumberAnimation { duration: 160; easing.type: Easing.OutCubic } }
  }
  Rectangle {
    anchors.centerIn: parent
    width: root.size * 1.7
    height: width
    radius: width / 2
    color: root.litColor
    opacity: root.lit ? 0.22 : 0
    Behavior on opacity { NumberAnimation { duration: 160; easing.type: Easing.OutCubic } }
  }
  Rectangle {
    anchors.fill: parent
    radius: width / 2
    color: root.lit ? root.litColor : root.offColor
    border.width: Math.max(1, root.size * 0.1)
    border.color: root.rim
    Behavior on color { ColorAnimation { duration: 160 } }

    // Specular dot, brighter when lit.
    Rectangle {
      x: parent.width * 0.26
      y: parent.height * 0.2
      width: parent.width * 0.3
      height: width
      radius: width / 2
      color: "#FFFFFF"
      opacity: root.lit ? 0.5 : 0.12
      Behavior on opacity { NumberAnimation { duration: 160 } }
    }
  }
}
