import QtQuick
import QtQuick.Layouts
import org.kde.plasma.plasmoid
import org.kde.plasma.plasma5support as P5Support
import org.kde.taskmanager as TaskManager

PlasmoidItem {
    id: root
    preferredRepresentation: fullRepresentation
    Plasmoid.backgroundHints: "NoBackground"
    readonly property var cfg: Plasmoid.configuration
    property var occupied: ({})

    TaskManager.VirtualDesktopInfo { id: vdi }
    TaskManager.ActivityInfo { id: activityInfo }
    TaskManager.TasksModel {
        id: tasks
        groupMode: TaskManager.TasksModel.GroupDisabled
        filterByVirtualDesktop: false
        filterByScreen: false
        filterByActivity: true
        activity: activityInfo.currentActivity
        onCountChanged: root.recount()
        onDataChanged: root.recount()
        onRowsInserted: root.recount()
        onRowsRemoved: root.recount()
    }

    // which desktops hold at least one normal window (windows on all desktops don't count)
    function recount() {
        const occ = {};
        for (let i = 0; i < tasks.count; ++i) {
            const idx = tasks.index(i, 0);
            if (!tasks.data(idx, TaskManager.AbstractTasksModel.IsWindow)) continue;
            if (tasks.data(idx, TaskManager.AbstractTasksModel.IsOnAllVirtualDesktops)) continue;
            const ds = tasks.data(idx, TaskManager.AbstractTasksModel.VirtualDesktops) || [];
            for (const d of ds) occ[d] = true;
        }
        occupied = occ;
    }
    Component.onCompleted: recount()

    P5Support.DataSource {
        id: exec
        engine: "executable"
        onNewData: (source) => disconnectSource(source)
    }
    function activate(n) {   // n is 1-based
        exec.connectSource("qdbus6 org.kde.KWin /KWin org.kde.KWin.setCurrentDesktop " + n);
    }
    function step(delta) {
        const ids = vdi.desktopIds, n = ids.length;
        const cur = ids.indexOf(vdi.currentDesktop);
        const next = Math.max(0, Math.min(n - 1, cur + delta));   // no wrap: predictable on a touchpad
        if (next !== cur) activate(next + 1);
    }

    fullRepresentation: MouseArea {
        Layout.minimumWidth: row.implicitWidth
        Layout.preferredWidth: row.implicitWidth
        Layout.fillHeight: true
        acceptedButtons: Qt.NoButton
        property real wheelAcc: 0
        onWheel: (wheel) => {          // touchpad: accumulate small deltas, one step per notch
            wheelAcc += wheel.angleDelta.y + wheel.angleDelta.x;
            if (Math.abs(wheelAcc) >= 120) { root.step(wheelAcc > 0 ? -1 : 1); wheelAcc = 0; }
        }

        Row {
            id: row
            height: parent.height
            Repeater {
                model: vdi.desktopIds
                delegate: MouseArea {
                    required property var modelData
                    required property int index
                    readonly property bool isActive: modelData === vdi.currentDesktop
                    readonly property bool isOccupied: root.occupied[modelData] === true
                    width: root.cfg.cellWidth
                    height: row.height
                    hoverEnabled: true
                    onClicked: root.activate(index + 1)

                    Text {
                        anchors.centerIn: parent
                        anchors.verticalCenterOffset: -1
                        text: index + 1
                        font.family: root.cfg.fontFamily
                        font.pointSize: root.cfg.fontSize
                        color: parent.isActive ? root.cfg.activeTextColor : root.cfg.textColor
                        opacity: parent.isActive || parent.isOccupied || parent.containsMouse ? 1 : 0.75
                    }
                    Rectangle {
                        anchors.bottom: parent.bottom
                        anchors.bottomMargin: Math.round(root.cfg.underline * 1.3)
                        width: parent.width
                        height: root.cfg.underline
                        color: parent.isActive ? root.cfg.activeColor : root.cfg.occupiedColor
                        visible: parent.isActive || parent.isOccupied
                    }
                }
            }
        }
    }
}
