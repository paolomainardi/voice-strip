import QtQuick

// Analog VU meter, drawn like the app's (crates/ui-kit/src/vu.rs) in the
// same 200 x 130 design space: scale arcs, ticks, labels, needle and
// pivot cap. `level` is the VU input before ballistics (dB relative to
// 0 VU); the needle runs the app's moving-coil model on every frame
// (crates/ui-kit/src/ballistics.rs), so it moves like the window's.
Item {
  id: root

  property var pal
  property real level: -Infinity
  property bool live: true
  // Run the movement only while the panel is open.
  property bool active: true
  property string fontFamily: ""

  // Needle position as a linear level (0 dB = 1.0) and its speed.
  property real position: 0
  property real speed: 0
  readonly property real needleDb: position > 0 ? 20 * Math.log10(position) : -60

  // ASA C16.5 movement: zeta 0.81272, natural frequency 13.512 rad/s
  // (99 % in 300 ms, 1.25 % overshoot), exact for an input held over dt.
  function advance(dt) {
    var zeta = 0.81272, wn = 13.512
    var sigma = zeta * wn
    var wd = wn * Math.sqrt(1 - zeta * zeta)
    var u = root.live && isFinite(root.level) && root.level > -60 ? Math.pow(10, root.level / 20) : 0
    var e = root.position - u
    var b = (root.speed + sigma * e) / wd
    var d = Math.exp(-sigma * dt)
    var c = Math.cos(wd * dt), s = Math.sin(wd * dt)
    var p = u + d * (e * c + b * s)
    var v = d * ((wd * b - sigma * e) * c - (wd * e + sigma * b) * s)
    var full = Math.pow(10, 3 / 20)
    if (p <= 0) { p = 0; v = Math.max(v, 0) }
    else if (p >= full) { p = full; v = Math.min(v, 0) }
    root.position = p
    root.speed = v
  }

  FrameAnimation {
    running: root.active && (root.live || root.position > 0)
    onTriggered: root.advance(Math.min(frameTime, 0.1))
  }

  readonly property real sx: width / 200
  readonly property real sy: height / 130

  implicitWidth: 200
  implicitHeight: 130

  // dB to needle angle (degrees, 0 = straight up), piecewise linear.
  readonly property var table: [
    [-20, -50], [-10, -30], [-7, -20], [-5, -12], [-3, -3],
    [-1, 8], [0, 15], [1, 24], [2, 35], [3, 50]
  ]
  function angleFor(v) {
    if (isNaN(v)) return -52
    // Below the scale the needle moves towards rest with the linear level.
    if (v <= -20) return -52 + Math.pow(10, (v + 20) / 20) * 2
    if (v >= 3) return 50
    for (var i = 0; i + 1 < table.length; i++) {
      var a = table[i], b = table[i + 1]
      if (v >= a[0] && v <= b[0]) return a[1] + (v - a[0]) / (b[0] - a[0]) * (b[1] - a[1])
    }
    return 0
  }

  onPalChanged: face.requestPaint()
  onFontFamilyChanged: face.requestPaint()
  onWidthChanged: face.requestPaint()

  Canvas {
    id: face
    anchors.fill: parent
    antialiasing: true
    renderStrategy: Canvas.Cooperative
    onPaint: {
      var ctx = getContext("2d")
      ctx.reset()
      if (!root.pal) return
      ctx.scale(root.sx, root.sy)
      var r = 8
      ctx.beginPath()
      ctx.roundedRect(1, 1, 198, 128, r, r)
      ctx.fillStyle = root.pal.vuFace
      ctx.fill()
      ctx.lineWidth = 2
      ctx.strokeStyle = root.pal.vuBorder
      ctx.stroke()

      function rad(deg) { return (deg - 90) * Math.PI / 180 }
      function polar(radius, deg) {
        var a = deg * Math.PI / 180
        return [100 + radius * Math.sin(a), 110 - radius * Math.cos(a)]
      }
      ctx.beginPath()
      ctx.arc(100, 110, 82, rad(-50), rad(15), false)
      ctx.lineWidth = 1.5
      ctx.strokeStyle = root.pal.vuInk
      ctx.stroke()
      ctx.beginPath()
      ctx.arc(100, 110, 82, rad(15), rad(50), false)
      ctx.lineWidth = 4
      ctx.strokeStyle = root.pal.red
      ctx.stroke()

      ctx.lineWidth = 1.5
      for (var i = 0; i < root.table.length; i++) {
        var t = root.table[i]
        var p0 = polar(82, t[1]), p1 = polar(72, t[1])
        ctx.beginPath()
        ctx.moveTo(p0[0], p0[1])
        ctx.lineTo(p1[0], p1[1])
        ctx.strokeStyle = t[0] >= 0 ? root.pal.red : root.pal.vuInk
        ctx.stroke()
      }

      var family = root.fontFamily !== "" ? "'" + root.fontFamily + "'" : "sans-serif"
      ctx.textAlign = "center"
      ctx.textBaseline = "alphabetic"
      ctx.font = "10px " + family
      ctx.fillStyle = root.pal.vuInk
      ctx.fillText("-20", 28, 50)
      ctx.fillText("-10", 53, 29)
      ctx.fillText("-5", 80, 18)
      ctx.fillText("-3", 95, 16)
      ctx.fillStyle = root.pal.red
      ctx.fillText("0", 124, 19)
      ctx.fillText("+3", 172, 50)
      ctx.fillStyle = root.pal.vuInk
      ctx.font = "16px " + family
      ctx.fillText("V U", 100, 92)
    }
  }

  // Needle and cap, clipped to the face.
  Item {
    x: root.sx
    y: root.sy
    width: 198 * root.sx
    height: 128 * root.sy
    clip: true

    Rectangle {
      id: needle
      width: Math.max(1, 1.6 * root.sx)
      height: 78 * root.sy
      x: 99 * root.sx - width / 2
      y: (112 - 78 - 1) * root.sy
      color: root.pal ? root.pal.needle : "black"
      antialiasing: true
      transformOrigin: Item.Bottom
      rotation: root.angleFor(root.needleDb)
    }

    Rectangle {
      width: 44 * root.sx
      height: width
      radius: width / 2
      x: 99 * root.sx - width / 2
      y: 128 * root.sy - width / 2
      color: root.pal ? root.pal.pivot : "black"
    }
  }
}
