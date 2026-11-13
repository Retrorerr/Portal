#!/usr/bin/python3 -I
"""Display inhibition for Portal's Plasma session, which runs without PowerDevil."""

import itertools
import os
import socket


SCREENSAVER = "org.freedesktop.ScreenSaver"
POWER_MANAGEMENT = "org.kde.Solid.PowerManagement"
POLICY_AGENT = POWER_MANAGEMENT + ".PolicyAgent"
BUS_NAME = "org.freedesktop.DBus"
BUS_PATH = "/org/freedesktop/DBus"
CHANGE_SCREEN_SETTINGS = 4

INTROSPECTION = f"""<node>
    <interface name="{SCREENSAVER}">
        <method name="Inhibit">
            <arg type="s" direction="in"/><arg type="s" direction="in"/>
            <arg type="u" direction="out"/>
        </method>
        <method name="UnInhibit"><arg type="u" direction="in"/></method>
    </interface>
    <interface name="{POLICY_AGENT}">
        <method name="AddInhibition">
            <arg type="u" direction="in"/><arg type="s" direction="in"/>
            <arg type="s" direction="in"/><arg type="u" direction="out"/>
        </method>
        <method name="ReleaseInhibition"><arg type="u" direction="in"/></method>
    </interface>
</node>"""


class Inhibitions:
    def __init__(self, address):
        self.address = address
        self.cookies = itertools.count(1)
        self.requests = {}
        self.connection = None

    def add(self, owner, display):
        if display and self.connection is None:
            connection = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            try:
                connection.connect(self.address)
            except OSError:
                connection.close()
                raise
            self.connection = connection
        cookie = next(self.cookies)
        self.requests[cookie] = owner, display
        return cookie

    def release(self, owner, cookie):
        request = self.requests.get(cookie)
        if request is not None and request[0] == owner:
            del self.requests[cookie]
            self.close_if_idle()

    def disconnected(self, owner):
        self.requests = {cookie: request for cookie, request in self.requests.items() if request[0] != owner}
        self.close_if_idle()

    def close_if_idle(self):
        if self.connection is not None and not any(display for _, display in self.requests.values()):
            self.connection.close()
            self.connection = None


def main():
    from gi.repository import Gio, GLib

    bus = Gio.bus_get_sync(Gio.BusType.SESSION, None)
    bus.set_exit_on_close(True)

    def request_name(name):
        # DO_NOT_QUEUE. Leave a name that another desktop service already owns alone.
        reply = bus.call_sync(BUS_NAME, BUS_PATH, BUS_NAME, "RequestName", GLib.Variant("(su)", (name, 4)),
                              None, Gio.DBusCallFlags.NONE, -1, None)
        return reply.unpack()[0] == 1  # PRIMARY_OWNER

    # The bus can activate several public names at once. One process serves them all.
    if not request_name("org.portal.ScreenInhibition"):
        return

    state = Inhibitions("\0" + os.environ["PORTAL_SCREEN_INHIBIT_SOCKET"])

    def method_call(connection, sender, path, interface, method, args, invocation):
        if method in ("Inhibit", "AddInhibition"):
            # KDE's portal uses bit 4 for display idle and bit 1 for suspend.
            display = method == "Inhibit" or bool(args.get_child_value(0).get_uint32() & CHANGE_SCREEN_SETTINGS)
            try:
                cookie = state.add(sender, display)
            except OSError as error:
                invocation.return_dbus_error("org.freedesktop.DBus.Error.Failed", str(error))
            else:
                invocation.return_value(GLib.Variant("(u)", (cookie,)))
        else:
            state.release(sender, args.get_child_value(0).get_uint32())
            invocation.return_value(None)

    def owner_changed(connection, sender, path, interface, signal, args):
        name, old_owner, new_owner = args.unpack()
        if name == old_owner and not new_owner:
            state.disconnected(name)

    interfaces = Gio.DBusNodeInfo.new_for_xml(INTROSPECTION)
    for path in ("/ScreenSaver", "/org/freedesktop/ScreenSaver"):
        bus.register_object(path, interfaces.lookup_interface(SCREENSAVER), method_call, None, None)
    bus.register_object("/org/kde/Solid/PowerManagement/PolicyAgent", interfaces.lookup_interface(POLICY_AGENT),
                        method_call, None, None)
    bus.signal_subscribe(BUS_NAME, BUS_NAME, "NameOwnerChanged", BUS_PATH, None, Gio.DBusSignalFlags.NONE, owner_changed)
    for name in (SCREENSAVER, POWER_MANAGEMENT, POLICY_AGENT):
        request_name(name)
    GLib.MainLoop().run()


if __name__ == "__main__":
    main()
