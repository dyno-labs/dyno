"""Open one URL in WebKitGTK, in a window the size of a phone's viewport, until stopped.

examples/webkit/open.sh runs it on a virtual display at the phone's pixel ratio. MiniBrowser
cannot size its window, and with no window manager it cannot go full screen either, so this
opens WebKit's view itself.

    WIDTH, HEIGHT    the viewport in CSS px (default 430 x 932, the iPhone 14 Pro Max)
    MEMORY_LIMIT_MB  a memory limit for WebKit's web process, which frees caches as it nears
                     the limit and is killed past it, as iOS kills a page (default: WebKit's own)
"""

import os
import sys

import gi

gi.require_version("Gtk", "3.0")
gi.require_version("WebKit2", "4.1")
# The versions must be chosen before this import.
from gi.repository import Gtk, WebKit2


def main() -> None:
    if len(sys.argv) != 2:
        sys.exit("usage: examples/webkit/open.py <url>")
    url = sys.argv[1]
    width = int(os.environ.get("WIDTH", "430"))
    height = int(os.environ.get("HEIGHT", "932"))

    # A fresh private profile for each run, as the CDP driver gives Chromium.
    context_properties = {
        "website_data_manager": WebKit2.WebsiteDataManager.new_ephemeral()
    }
    memory_limit = os.environ.get("MEMORY_LIMIT_MB")
    if memory_limit:
        settings = WebKit2.MemoryPressureSettings.new()
        settings.set_memory_limit(int(memory_limit))
        context_properties["memory_pressure_settings"] = settings
    context = WebKit2.WebContext(**context_properties)

    view = WebKit2.WebView.new_with_context(context)
    view_settings = view.get_settings()
    view_settings.set_enable_webgl(True)
    view_settings.set_media_playback_requires_user_gesture(False)

    window = Gtk.Window()
    window.set_default_size(width, height)
    window.set_resizable(False)
    window.move(0, 0)
    window.add(view)
    window.connect("destroy", Gtk.main_quit)
    window.show_all()
    view.load_uri(url)
    Gtk.main()


if __name__ == "__main__":
    main()
