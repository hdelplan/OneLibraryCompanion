#!/usr/bin/python3
"""OLC's GTK/WebKit window. Application features remain in the shared host/UI."""
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import urllib.request
import uuid

import gi

gi.require_version("Gtk", "3.0")
gi.require_version("WebKit2", "4.1")
from gi.repository import Gio, GLib, Gtk, WebKit2


class OLC(Gtk.Application):
    def __init__(self):
        super().__init__(application_id="org.onelibrarycompanion.desktop", flags=Gio.ApplicationFlags.FLAGS_NONE)
        self.window = None
        self.child = None
        self.log = None
        self.instance = str(uuid.uuid4())
        self.starting = True
        self.attempts = 0
        self.origin = "http://127.0.0.1:8787"
        self.autostart = Path(os.environ.get("XDG_CONFIG_HOME", str(Path.home() / ".config"))) / "autostart/onelibrarycompanion.desktop"

    def do_activate(self):
        if self.window:
            self.window.show_all()
            self.window.present()
            return
        self.hold()  # Closing a window does not stop the LAN service.
        self.window = Gtk.ApplicationWindow(application=self, title="OneLibraryCompanion")
        self.window.set_default_size(1280, 830)
        self.window.connect("delete-event", self.hide_window)
        box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL)
        self.window.add(box)
        bar = Gtk.MenuBar()
        app_menu = Gtk.Menu()
        root = Gtk.MenuItem(label="OLC")
        root.set_submenu(app_menu)
        bar.append(root)
        login = Gtk.CheckMenuItem(label="Open at Login")
        login.set_active(self.autostart.exists())
        login.connect("toggled", self.toggle_login)
        app_menu.append(login)
        full = Gtk.CheckMenuItem(label="Full Screen")
        full.connect("toggled", lambda item: self.window.fullscreen() if item.get_active() else self.window.unfullscreen())
        app_menu.append(full)
        reload_item = Gtk.MenuItem(label="Reload Interface")
        reload_item.connect("activate", lambda _: self.web.reload_bypass_cache())
        app_menu.append(reload_item)
        quit_item = Gtk.MenuItem(label="Quit OLC")
        quit_item.connect("activate", lambda _: self.quit())
        app_menu.append(quit_item)
        box.pack_start(bar, False, False, 0)
        # A dedicated persistent WebKit store avoids sharing cookies or preferences with browsers.
        data = Path(os.environ.get("XDG_DATA_HOME", str(Path.home() / ".local/share"))) / "onelibrarycompanion"
        cache = Path(os.environ.get("XDG_CACHE_HOME", str(Path.home() / ".cache"))) / "onelibrarycompanion"
        manager = WebKit2.WebsiteDataManager(base_data_directory=str(data / "webview"), base_cache_directory=str(cache))
        context = WebKit2.WebContext.new_with_website_data_manager(manager)
        context.connect("download-started", self.download_started)
        self.web = WebKit2.WebView.new_with_context(context)
        self.web.connect("decide-policy", self.decide_policy)
        box.pack_start(self.web, True, True, 0)
        self.window.show_all()
        self.web.load_html("<html style='background:#111;color:white;font:24px sans-serif'><body>Starting OneLibraryCompanion…</body></html>", None)
        try:
            env = os.environ.copy()
            base = Path(__file__).resolve().parent
            env.setdefault("OLC_UI_ROOT", str(base / "dist"))
            env.setdefault("OLC_BIND", env.get("PIONEER_COMPANION_BIND", "0.0.0.0:8787"))
            env["OLC_INSTANCE"] = self.instance
            port = int(env["OLC_BIND"].rsplit(":", 1)[1])
            if not 1 <= port <= 65535:
                raise ValueError("OLC_BIND must use a fixed port from 1 to 65535")
            self.origin = f"http://127.0.0.1:{port}"
            data.mkdir(parents=True, exist_ok=True)
            self.log = (data / "host.log").open("w")
            self.child = subprocess.Popen([str(base / "olc-host")], env=env, stdout=self.log, stderr=self.log)
            GLib.timeout_add(250, self.check_host)
        except Exception as error:
            self.fail(str(error))

    def check_host(self):
        if self.child.poll() is not None:
            self.fail("The OLC service stopped. Another service may already be using its port. See ~/.local/share/onelibrarycompanion/host.log.")
            return False
        if self.starting:
            self.attempts += 1
            try:
                with urllib.request.urlopen(self.origin + "/api/health", timeout=0.2) as response:
                    ready = json.load(response).get("instance") == self.instance
                if ready:
                    self.starting = False
                    self.web.load_uri(self.origin)
            except (OSError, ValueError):
                pass
            if self.attempts > 120:
                self.fail("OLC did not become ready. Check host.log and restart the application.")
                return False
        return True

    def hide_window(self, window, _event):
        window.hide()
        return True

    def toggle_login(self, item):
        try:
            if item.get_active():
                self.autostart.parent.mkdir(parents=True, exist_ok=True)
                self.autostart.write_text("[Desktop Entry]\nType=Application\nName=OneLibraryCompanion\nExec=/usr/bin/olc\nTerminal=false\n")
            else:
                self.autostart.unlink(missing_ok=True)
        except OSError as error:
            dialog = Gtk.MessageDialog(transient_for=self.window, modal=True, message_type=Gtk.MessageType.ERROR, buttons=Gtk.ButtonsType.OK, text="Unable to change login startup")
            dialog.format_secondary_text(str(error))
            dialog.run()
            dialog.destroy()

    def decide_policy(self, _web, decision, kind):
        if kind == WebKit2.PolicyDecisionType.RESPONSE:
            if not decision.is_mime_type_supported():
                decision.download()
                return True
        if kind in (WebKit2.PolicyDecisionType.NAVIGATION_ACTION, WebKit2.PolicyDecisionType.NEW_WINDOW_ACTION):
            uri = decision.get_navigation_action().get_request().get_uri()
            if uri.startswith("mailto:"):
                Gio.AppInfo.launch_default_for_uri(uri, None)
                decision.ignore()
                return True
            if uri.startswith(self.origin + "/") or uri == self.origin or uri.startswith("blob:" + self.origin + "/") or uri == "about:blank":
                return False
            decision.ignore()
            return True
        return False

    def download_started(self, _context, download):
        download.connect("decide-destination", self.download_destination)
        download.connect("failed", lambda _d, error: self.download_error(error.message))

    def download_error(self, message):
        # Cancellation is a normal user action.
        if "cancel" in message.lower():
            return
        dialog = Gtk.MessageDialog(transient_for=self.window, modal=True, message_type=Gtk.MessageType.ERROR, buttons=Gtk.ButtonsType.OK, text="Download failed")
        dialog.format_secondary_text(message)
        dialog.run()
        dialog.destroy()

    def download_destination(self, download, suggested):
        dialog = Gtk.FileChooserDialog(title="Save export", transient_for=self.window, action=Gtk.FileChooserAction.SAVE)
        dialog.add_buttons("Cancel", Gtk.ResponseType.CANCEL, "Save", Gtk.ResponseType.ACCEPT)
        dialog.set_current_name(Path(suggested).name)
        dialog.set_do_overwrite_confirmation(True)
        if dialog.run() == Gtk.ResponseType.ACCEPT:
            download.set_allow_overwrite(True)
            download.set_destination(Path(dialog.get_filename()).as_uri())
        else:
            download.cancel()
        dialog.destroy()
        return True

    def fail(self, message):
        self.window.show_all()
        dialog = Gtk.MessageDialog(transient_for=self.window, modal=True, message_type=Gtk.MessageType.ERROR, buttons=Gtk.ButtonsType.CLOSE, text="Unable to run OneLibraryCompanion")
        dialog.format_secondary_text(message)
        dialog.run()
        dialog.destroy()
        self.quit()

    def do_shutdown(self):
        if self.child and self.child.poll() is None:
            self.child.terminate()
            try:
                self.child.wait(timeout=3)
            except subprocess.TimeoutExpired:
                self.child.kill()
                self.child.wait()
        if self.log:
            self.log.close()
        Gtk.Application.do_shutdown(self)


if __name__ == "__main__":
    app = OLC()
    GLib.unix_signal_add(GLib.PRIORITY_DEFAULT, signal.SIGTERM, lambda: app.quit())
    GLib.unix_signal_add(GLib.PRIORITY_DEFAULT, signal.SIGINT, lambda: app.quit())
    sys.exit(app.run(sys.argv))
