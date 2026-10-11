import QtQuick
import Quickshell.Io
import qs.Commons
import qs.Ui

// Mijia air conditioner, driven by the `aircon` CLI (miIO over the LAN).
// The CLI path can be overridden with the bar entry setting "command".
// Bar: snowflake + target temperature while running, dimmed when off.
//   click opens the panel, right-click toggles power, scroll nudges the target by 0.5°.
// Writes are optimistic: the UI updates at once (including the AC's own
// interlocks, see predict()), then a read-back corrects it.
Panel {
  id: root
  moduleName: "xiaomi.aircon"
  ipcTarget: "xiaomi.aircon"

  readonly property string cli: setting("command", "aircon")
  readonly property string icon: "󰜗"
  readonly property var modes: [
    { value: "cool", label: "Cool", icon: "󰜗" },
    { value: "dry", label: "Dry", icon: "󰖌" },
    { value: "fan", label: "Fan", icon: "󰈐" }
  ]
  readonly property var airflows: [
    { value: "upward", label: "Upward", icon: "󰁝" },
    { value: "downward", label: "Downward", icon: "󰁅" },
    { value: "circulate", label: "Circulate", icon: "󰑖" }
  ]
  // coolingOnly: hidden in fan mode, where the compressor is off.
  readonly property var comfortModes: [
    { key: "ai_energy_saving", label: "AI saving", icon: "󰌪", coolingOnly: true },
    { key: "turbo", label: "Turbo", icon: "󱐋", coolingOnly: true },
    { key: "gentle", label: "Gentle", icon: "󰖝", coolingOnly: false },
    { key: "sleep", label: "Sleep", icon: "󰒲", coolingOnly: false }
  ]
  readonly property var unitSettings: [
    { key: "display", label: "Display", icon: "󰛨" },
    { key: "beep", label: "Beep", icon: "󰂞" },
    { key: "dryer", label: "Self-dry", icon: "󰖌" }
  ]
  // Fixed louvre directions, drawn as one arrow rotated to the louvre angle:
  // horizontal seen from above (↑ fanned left to right), vertical from the
  // side (→ tilted up to down).
  readonly property var horizontalDirections: [
    { value: "left", tooltip: "Left", angle: -60 },
    { value: "left-center", tooltip: "Left of centre", angle: -30 },
    { value: "center", tooltip: "Centre", angle: 0 },
    { value: "right-center", tooltip: "Right of centre", angle: 30 },
    { value: "right", tooltip: "Right", angle: 60 }
  ]
  readonly property var verticalDirections: [
    { value: "up", tooltip: "Up", angle: -50 },
    { value: "up-center", tooltip: "Above centre", angle: -25 },
    { value: "center", tooltip: "Centre", angle: 0 },
    { value: "down-center", tooltip: "Below centre", angle: 25 },
    { value: "down", tooltip: "Down", angle: 50 }
  ]

  property var ac: ({})
  property bool reachable: false
  property bool loaded: false
  property var pending: ({})
  property real wheelAccumulator: 0
  property bool showMore: false

  readonly property bool powered: ac.power === true
  readonly property real target: ac.target_temperature !== undefined ? ac.target_temperature : 0
  // Fan mode only moves air: no target temperature, no compressor features.
  readonly property bool cooling: ac.mode !== "fan"
  readonly property bool turbo: ac.turbo === true
  readonly property bool gentle: ac.gentle === "on"
  readonly property bool faultActive: typeof ac.fault === "string" && ac.fault !== "none"

  function degrees(value) {
    if (value === undefined || value === null) return "—"
    return (Math.round(value * 10) / 10) + "°"
  }

  // gentle is an off/on enum on the AC; the other switches are booleans.
  function isOn(key) {
    return ac[key] === true || ac[key] === "on"
  }

  function switchValue(key, on) {
    return key === "gentle" ? (on ? "on" : "off") : on
  }

  function change(key, value) {
    var values = {}
    values[key] = value
    return values
  }

  function modeLabel() {
    for (var i = 0; i < modes.length; i++)
      if (modes[i].value === ac.mode) return modes[i].label
    return ""
  }

  function statusLine() {
    if (!loaded) return "Connecting"
    if (!reachable) return "Unreachable"
    var room = "Room " + degrees(ac.room_temperature)
    if (!powered) return "Off · " + room
    return modeLabel() + (turbo ? " · Turbo" : "") + " · " + room
  }

  function fanLabel() {
    if (turbo) return "Turbo"
    if (ac.fan_speed === "auto") return "Auto"
    return ac.fan_speed ? "Level " + ac.fan_speed : "—"
  }

  function lowWattLabel() {
    return !ac.low_watt || ac.low_watt === "off" ? "Off" : "L" + ac.low_watt
  }

  // L1 draws the least power, so the slider runs Off, L6 … L1: further right
  // is a tighter cap. Slider position and level map onto each other by 7 - n
  // (0 stays Off), so the same function converts both ways.
  function flipLowWatt(n) {
    return n === 0 ? 0 : 7 - n
  }

  function refresh() {
    if (!statusProc.running && !setProc.running) statusProc.running = true
  }

  function parseStatus(text) {
    loaded = true
    try {
      var next = JSON.parse(text)
      // Keep pending optimistic values until their write lands.
      ac = Object.assign(next, pending)
      reachable = true
    } catch (e) {
      reachable = false
    }
  }

  // The AC's own interlocks, mirrored so the UI does not flicker while a
  // write is in flight (see docs/protocol.md for how they were observed).
  function predict(values) {
    var effects = {}
    if (values.turbo === true) effects.fan_speed = "7"
    if (values.turbo === false) effects.fan_speed = "auto"
    if (values.fan_speed !== undefined) effects.turbo = false
    if (values.gentle === "on") effects.airflow = "off"
    // Airflow patterns drive the louvres themselves: upward/downward take the
    // vertical louvre, circulate takes both. Any manual louvre setting ends the pattern.
    if (values.airflow !== undefined && values.airflow !== "off") {
      effects.gentle = "off"
      effects.vertical = "off"
      if (values.airflow === "circulate") effects.horizontal = "off"
    }
    if ((values.vertical !== undefined && values.vertical !== "off")
        || (values.horizontal !== undefined && values.horizontal !== "off"))
      effects.airflow = "off"
    return Object.assign(effects, values)
  }

  // Queue a write and show it immediately; flushed in one `aircon set` call.
  function apply(values) {
    pending = Object.assign({}, pending, values)
    ac = Object.assign({}, ac, predict(values))
    writeDelay.restart()
  }

  function flush() {
    var keys = Object.keys(pending)
    if (keys.length === 0 || setProc.running) return
    var args = [cli, "set"]
    for (var i = 0; i < keys.length; i++) {
      var value = pending[keys[i]]
      if (value === true) value = "on"
      else if (value === false) value = "off"
      args.push(keys[i] + "=" + value)
    }
    pending = ({})
    setProc.command = args
    setProc.running = true
  }

  function setTarget(value) {
    apply({ target_temperature: Math.max(16, Math.min(31, Math.round(value * 2) / 2)) })
  }

  // Segmented choices: picking the active option again turns it off.
  function choose(key, value) {
    apply(change(key, ac[key] === value ? "off" : value))
  }

  Component.onCompleted: refresh()
  onOpenedChanged: if (opened) refresh()

  Process {
    id: statusProc
    command: [root.cli, "status", "--json"]
    stdout: StdioCollector { waitForEnd: true; onStreamFinished: root.parseStatus(text) }
  }

  Process {
    id: setProc
    stderr: StdioCollector { id: setErrors; waitForEnd: true }
    onExited: function(exitCode) {
      if (exitCode !== 0) console.warn("xiaomi.aircon: " + setErrors.text)
      if (Object.keys(root.pending).length > 0) root.flush()
      else root.refresh()
    }
  }

  // Coalesces scroll/click bursts into a single write.
  Timer { id: writeDelay; interval: 350; onTriggered: root.flush() }
  Timer { interval: root.opened ? 4000 : 30000; running: true; repeat: true; onTriggered: root.refresh() }

  implicitWidth: button.implicitWidth
  implicitHeight: button.implicitHeight

  BarIconButton {
    id: button
    anchors.fill: parent
    bar: root.bar
    readonly property bool showTarget: root.powered && root.cooling && !vertical
    text: showTarget ? root.icon + " " + root.degrees(root.target) : root.icon
    slotSize: Style.bar.iconSlot * (showTarget ? 2.4 : 1)
    dimmed: !root.powered
    tooltipText: root.opened ? "" : "AC: " + root.statusLine()
    onPressed: function(b) {
      if (b === Qt.RightButton) {
        if (root.reachable) root.apply({ power: !root.powered })
      } else {
        root.toggle()
      }
    }
    onWheelMoved: function(delta) {
      if (!root.powered || !root.cooling) return
      var wheel = Util.wheelSteps(root.wheelAccumulator, delta)
      root.wheelAccumulator = wheel.remainder
      if (wheel.steps !== 0) root.setTarget(root.target + wheel.steps * 0.5)
    }
  }

  KeyboardPanel {
    id: panel
    anchorItem: button
    owner: root
    bar: root.bar
    open: root.opened
    focusTarget: keyCatcher
    contentWidth: panel.fittedContentWidth(Style.space(400))
    contentHeight: panel.fittedContentHeight(column.implicitHeight)

    PanelKeyCatcher {
      id: keyCatcher
      anchors.fill: parent
      // Up/down (or k/j) nudge the temperature. Power is deliberately not on a
      // key: the panel grabs focus, and a stray Enter must not switch the AC off.
      onMoveRequested: function(dx, dy) {
        if (dy !== 0 && root.powered && root.cooling) root.setTarget(root.target - dy * 0.5)
      }
      onCloseRequested: root.close()
      onTabRequested: function(direction) { root.switchPanel(direction) }

      // Scrolls only when the panel is taller than the screen allows.
      Flickable {
        anchors.fill: parent
        contentWidth: width
        contentHeight: column.implicitHeight
        interactive: contentHeight > height
        clip: true
        boundsBehavior: Flickable.StopAtBounds

        Column {
          id: column
          width: parent.width
          spacing: Style.space(14)

          // ---------- Hero: icon · name/status · power switch ----------
          Item {
            width: parent.width
            implicitHeight: Math.max(heroIcon.implicitHeight, heroLabels.implicitHeight, powerSwitch.implicitHeight)

            Text {
              id: heroIcon
              text: root.icon
              color: root.bar.foreground
              opacity: root.powered ? 1 : 0.45
              font.family: root.bar.fontFamily
              font.pixelSize: Style.font.display
              anchors.left: parent.left
              anchors.verticalCenter: parent.verticalCenter
            }

            Column {
              id: heroLabels
              anchors.left: heroIcon.right
              anchors.leftMargin: Style.space(14)
              anchors.right: powerSwitch.left
              anchors.rightMargin: Style.space(10)
              anchors.verticalCenter: parent.verticalCenter
              spacing: Style.space(2)

              Text {
                text: "Air Conditioner"
                color: root.bar.foreground
                font.family: root.bar.fontFamily
                font.pixelSize: Style.font.title
                font.bold: true
                elide: Text.ElideRight
                width: parent.width
              }

              Text {
                textFormat: Text.PlainText
                text: root.statusLine().toUpperCase()
                color: Qt.darker(root.bar.foreground, 1.4)
                font.family: root.bar.fontFamily
                font.pixelSize: Style.font.caption
                font.bold: true
                font.letterSpacing: 1.2
                elide: Text.ElideRight
                width: parent.width
              }
            }

            ToggleSwitch {
              id: powerSwitch
              anchors.right: parent.right
              anchors.verticalCenter: parent.verticalCenter
              checked: root.powered
              interactive: root.reachable
              foreground: root.bar.foreground
              onToggled: root.apply({ power: !root.powered })
            }
          }

          // Everything below the hero only makes sense while the AC runs.
          Column {
            width: parent.width
            spacing: Style.space(16)
            visible: root.powered

            // ---------- Target temperature (not in fan mode) ----------
            Item {
              width: parent.width
              implicitHeight: targetText.implicitHeight
              visible: root.cooling

              Button {
                anchors.left: parent.left
                anchors.verticalCenter: parent.verticalCenter
                iconText: "󰍴"
                foreground: root.bar.foreground
                bordered: true
                onClicked: root.setTarget(root.target - 0.5)
              }

              Text {
                id: targetText
                anchors.centerIn: parent
                text: root.degrees(root.target)
                color: root.bar.foreground
                font.family: root.bar.fontFamily
                font.pixelSize: Style.font.displayLarge
                font.bold: true
              }

              Button {
                anchors.right: parent.right
                anchors.verticalCenter: parent.verticalCenter
                iconText: "󰐕"
                foreground: root.bar.foreground
                bordered: true
                onClicked: root.setTarget(root.target + 0.5)
              }
            }

            // ---------- Mode ----------
            Section {
              title: "MODE"

              ButtonRow {
                model: root.modes
                delegate: Chip {
                  required property var modelData
                  width: parent.cellWidth
                  iconText: modelData.icon
                  text: modelData.label
                  active: root.ac.mode === modelData.value
                  onClicked: root.apply({ mode: modelData.value })
                }
              }
            }

            // ---------- Fan speed: slider 1-7 plus Auto ----------
            Section {
              title: "FAN SPEED"
              value: root.fanLabel()

              Item {
                width: parent.width
                implicitHeight: Math.max(fanSlider.implicitHeight, autoButton.implicitHeight)

                PanelSlider {
                  id: fanSlider
                  bar: root.bar
                  anchors.left: parent.left
                  anchors.right: autoButton.left
                  anchors.rightMargin: Style.space(12)
                  anchors.verticalCenter: parent.verticalCenter
                  minimum: 1
                  maximum: 7
                  step: 1
                  integer: true
                  tickCount: 7
                  // In auto the knob rests at 1, dimmed; dragging picks a fixed speed.
                  opacity: root.ac.fan_speed === "auto" && !root.turbo ? 0.45 : 1
                  value: root.turbo ? 7 : (parseInt(root.ac.fan_speed) || 1)
                  onReleased: function(v) { root.apply({ fan_speed: String(Math.round(v)) }) }
                }

                Chip {
                  id: autoButton
                  anchors.right: parent.right
                  anchors.verticalCenter: parent.verticalCenter
                  width: Style.space(64)
                  text: "Auto"
                  active: root.ac.fan_speed === "auto" && !root.turbo
                  onClicked: root.apply({ fan_speed: "auto" })
                }
              }
            }

            // ---------- Comfort modes ----------
            Section {
              title: "COMFORT"

              Grid {
                id: comfortGrid
                width: parent.width
                columns: 2
                spacing: Style.space(6)
                readonly property real cellWidth: (width - spacing * (columns - 1)) / columns

                Repeater {
                  model: root.comfortModes
                  Chip {
                    required property var modelData
                    width: comfortGrid.cellWidth
                    visible: root.cooling || !modelData.coolingOnly
                    iconText: modelData.icon
                    text: modelData.label
                    active: root.isOn(modelData.key)
                    onClicked: root.apply(root.change(modelData.key, root.switchValue(modelData.key, !root.isOn(modelData.key))))
                  }
                }
              }
            }

            // ---------- Low watt: slider Off, L6 … L1 (compressor modes only) ----------
            Section {
              title: "LOW WATT"
              value: root.lowWattLabel()
              visible: root.cooling

              PanelSlider {
                bar: root.bar
                width: parent.width
                minimum: 0
                maximum: 6
                step: 1
                integer: true
                tickCount: 7
                value: root.flipLowWatt(parseInt(root.ac.low_watt) || 0)
                onReleased: function(v) {
                  var level = root.flipLowWatt(Math.round(v))
                  root.apply({ low_watt: level === 0 ? "off" : String(level) })
                }
              }
            }

            // ---------- Airflow ----------
            Section {
              title: "AIRFLOW"
              value: root.gentle ? "Paused by Gentle" : ""

              ButtonRow {
                model: root.airflows
                opacity: root.gentle ? 0.5 : 1
                delegate: Chip {
                  required property var modelData
                  width: parent.cellWidth
                  iconText: modelData.icon
                  text: modelData.label
                  active: root.ac.airflow === modelData.value
                  onClicked: root.choose("airflow", modelData.value)
                }
              }
            }

            // ---------- Louvres: swing or a fixed position ----------
            Section {
              title: "LOUVRES"

              LouvreRow { label: "↔"; key: "horizontal"; glyph: "󰁝"; directions: root.horizontalDirections }
              LouvreRow { label: "↕"; key: "vertical"; glyph: "󰁔"; directions: root.verticalDirections }
            }
          }

          PanelSeparator { foreground: root.bar.foreground }

          // ---------- Readings ----------
          Row {
            width: parent.width
            spacing: Style.space(20)

            Column {
              width: (parent.width - parent.spacing) / 2
              spacing: Style.spacing.labelGap
              InfoPair { label: "Room"; value: root.degrees(root.ac.room_temperature) }
              InfoPair { label: "Outdoor"; value: root.degrees(root.ac.outdoor_temperature) }
              InfoPair { label: "Fault"; value: root.ac.fault || "—"; visible: root.faultActive }
            }

            Column {
              width: (parent.width - parent.spacing) / 2
              spacing: Style.spacing.labelGap
              InfoPair { label: "Compressor"; value: root.ac.compressor_frequency !== undefined ? root.ac.compressor_frequency + " Hz" : "—" }
              InfoPair { label: "Energy"; value: root.ac.energy !== undefined ? root.ac.energy.toFixed(1) + " kWh" : "—" }
            }
          }

          // ---------- More: unit settings, collapsed by default ----------
          Column {
            width: parent.width
            spacing: Style.space(10)

            Item {
              width: parent.width
              implicitHeight: moreHeader.implicitHeight

              PanelSectionHeader {
                id: moreHeader
                text: (root.showMore ? "󰅀 " : "󰅂 ") + "MORE"
                foreground: root.bar.foreground
                fontFamily: root.bar.fontFamily
              }

              MouseArea {
                anchors.fill: parent
                cursorShape: Qt.PointingHandCursor
                onClicked: root.showMore = !root.showMore
              }
            }

            Grid {
              id: moreGrid
              visible: root.showMore
              width: parent.width
              columns: 3
              spacing: Style.space(6)
              readonly property real cellWidth: (width - spacing * (columns - 1)) / columns

              Repeater {
                model: root.unitSettings
                Chip {
                  required property var modelData
                  width: moreGrid.cellWidth
                  iconText: modelData.icon
                  text: modelData.label
                  active: root.isOn(modelData.key)
                  onClicked: root.apply(root.change(modelData.key, !root.isOn(modelData.key)))
                }
              }
            }
          }
        }
      }
    }
  }

  // A titled block; `value` shows the current setting on the right of the title.
  component Section: Column {
    id: section
    property string title: ""
    property string value: ""
    width: parent.width
    spacing: Style.space(10)

    Item {
      width: parent.width
      implicitHeight: header.implicitHeight

      PanelSectionHeader {
        id: header
        text: section.title
        foreground: root.bar.foreground
        fontFamily: root.bar.fontFamily
      }

      PanelSectionHeader {
        anchors.right: parent.right
        text: section.value.toUpperCase()
        foreground: Qt.lighter(root.bar.foreground, 1.6)
        fontFamily: root.bar.fontFamily
      }
    }
  }

  // The stronger "selected" style (accent text) makes the current choice
  // readable at a glance; the plain active fill is too subtle on dark themes.
  component Chip: Button {
    selected: active
    fontSize: Style.font.bodySmall
    foreground: root.bar.foreground
    fontFamily: root.bar.fontFamily
    bordered: true
  }

  // Equal-width buttons across the panel; delegates size themselves from cellWidth.
  component ButtonRow: Row {
    id: row
    property var model: []
    property Component delegate: null
    readonly property real cellWidth: model.length > 0 ? (width - spacing * (model.length - 1)) / model.length : 0
    width: parent.width
    spacing: Style.space(6)

    Repeater {
      model: row.model
      delegate: row.delegate
    }
  }

  // One louvre axis: [Swing] then five direction arrows; tapping the active
  // choice turns it off.
  component LouvreRow: Row {
    id: louvre
    property string label: ""
    property string key: ""
    property string glyph: ""
    property var directions: []
    readonly property real labelWidth: Style.space(18)
    readonly property real swingWidth: Style.space(80)
    readonly property real directionWidth: (width - labelWidth - swingWidth - spacing * 6) / 5
    width: parent.width
    spacing: Style.space(6)

    Text {
      width: louvre.labelWidth
      anchors.verticalCenter: parent.verticalCenter
      text: louvre.label
      color: root.bar.foreground
      opacity: 0.6
      font.family: root.bar.fontFamily
      font.pixelSize: Style.font.title
    }

    Chip {
      width: louvre.swingWidth
      iconText: "󰑖"
      text: "Swing"
      active: root.ac[louvre.key] === "swing"
      onClicked: root.choose(louvre.key, "swing")
    }

    Repeater {
      model: louvre.directions
      Chip {
        required property var modelData
        width: louvre.directionWidth
        iconText: louvre.glyph
        iconRotation: modelData.angle
        tooltipText: modelData.tooltip
        active: root.ac[louvre.key] === modelData.value
        onClicked: root.choose(louvre.key, modelData.value)
      }
    }
  }

  component InfoPair: Row {
    property string label: ""
    property string value: ""

    width: parent.width
    spacing: Style.space(8)

    InfoLabel { text: label }
    Item { width: Math.max(0, parent.width - parent.children[0].implicitWidth - parent.children[2].implicitWidth - parent.spacing * 2); height: 1 }
    InfoValue { text: value }
  }

  component InfoLabel: Text {
    textFormat: Text.PlainText
    color: root.bar.foreground
    opacity: 0.6
    font.family: root.bar.fontFamily
    font.pixelSize: Style.font.bodySmall
  }

  component InfoValue: Text {
    textFormat: Text.PlainText
    color: root.bar.foreground
    font.family: root.bar.fontFamily
    font.pixelSize: Style.font.bodySmall
  }
}
