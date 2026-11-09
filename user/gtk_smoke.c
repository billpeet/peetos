/*
 * GTK 3 smoke client for PeetOS: a real GTK window under peetwl (Wayland
 * backend, cairo into wl_shm), driven by scripts/gtk_smoke_driver.py with
 * real PS/2 key presses and pointer clicks through QEMU.
 *
 * Markers on stderr (the serial console):
 *   [gtk-smoke] start (GTK x.y.z, display <type>)
 *   [gtk-smoke] first frame drawn
 *   [gtk-smoke] widget <name> at <x>,<y> <w>x<h>   (window coordinates)
 *   [gtk-smoke] entry text '<text>'                (on every change)
 *   [gtk-smoke] button clicked with entry '<text>'
 *   [gtk-smoke] exiting
 *
 * The window is undecorated: GTK 3 only uses server-side decorations through
 * the KDE protocol, which peetwl does not offer, and peetwl already frames
 * the window. That also makes window and surface coordinates the same.
 */
#include <gdk/gdkwayland.h>
#include <gtk/gtk.h>
#include <math.h>
#include <stdio.h>
#include <string.h>

static GtkWidget *entry, *button, *progress, *check;
static int reported;

static void say(const char *fmt, ...) G_GNUC_PRINTF(1, 2);
static void say(const char *fmt, ...) {
    va_list ap;
    va_start(ap, fmt);
    fputs("[gtk-smoke] ", stderr);
    vfprintf(stderr, fmt, ap);
    fputc('\n', stderr);
    va_end(ap);
    fflush(stderr);
}

static void report_widget(GtkWidget *w, GtkWidget *toplevel, const char *name) {
    int x = 0, y = 0;
    GtkAllocation a;
    gtk_widget_get_allocation(w, &a);
    gtk_widget_translate_coordinates(w, toplevel, 0, 0, &x, &y);
    say("widget %s at %d,%d %dx%d", name, x, y, a.width, a.height);
}

static gboolean on_window_draw(GtkWidget *win, cairo_t *cr, gpointer data) {
    (void)cr;
    (void)data;
    if (!reported) {
        reported = 1;
        say("first frame drawn");
        report_widget(entry, win, "entry");
        report_widget(button, win, "button");
    }
    return FALSE;
}

static gboolean draw_art(GtkWidget *area, cairo_t *cr, gpointer data) {
    (void)data;
    int w = gtk_widget_get_allocated_width(area), h = gtk_widget_get_allocated_height(area);
    cairo_pattern_t *g = cairo_pattern_create_linear(0, 0, w, 0);
    cairo_pattern_add_color_stop_rgb(g, 0, 0.21, 0.52, 0.89);
    cairo_pattern_add_color_stop_rgb(g, 1, 0.56, 0.35, 0.80);
    cairo_rectangle(cr, 0, 0, w, h);
    cairo_set_source(cr, g);
    cairo_fill(cr);
    cairo_pattern_destroy(g);
    for (int i = 0; i < 5; i++) {
        cairo_arc(cr, 30 + i * (w - 60) / 4.0, h / 2.0, h / 3.0, 0, 2 * G_PI);
        cairo_set_source_rgba(cr, 1, 1, 1, 0.25 + 0.12 * i);
        cairo_fill(cr);
    }
    return FALSE;
}

static void on_entry_changed(GtkEditable *e, gpointer data) {
    (void)data;
    const char *text = gtk_entry_get_text(GTK_ENTRY(e));
    say("entry text '%s'", text);
    gtk_progress_bar_set_fraction(GTK_PROGRESS_BAR(progress), MIN(1.0, strlen(text) / 5.0));
}

static gboolean quit_later(gpointer data) {
    (void)data;
    say("exiting");
    gtk_main_quit();
    return G_SOURCE_REMOVE;
}

static void on_clicked(GtkButton *b, gpointer data) {
    (void)b;
    (void)data;
    say("button clicked with entry '%s'", gtk_entry_get_text(GTK_ENTRY(entry)));
    gtk_toggle_button_set_active(GTK_TOGGLE_BUTTON(check), TRUE);
    gtk_button_set_label(GTK_BUTTON(button), "Clicked!");
    /* Leave the final frame up for the driver's screendump. */
    g_timeout_add(2500, quit_later, NULL);
}

int main(int argc, char **argv) {
    gtk_init(&argc, &argv);
    GdkDisplay *display = gdk_display_get_default();
    say("start (GTK %u.%u.%u, display %s)", gtk_get_major_version(), gtk_get_minor_version(),
        gtk_get_micro_version(), GDK_IS_WAYLAND_DISPLAY(display) ? "wayland" : G_OBJECT_TYPE_NAME(display));

    GtkWidget *win = gtk_window_new(GTK_WINDOW_TOPLEVEL);
    gtk_window_set_title(GTK_WINDOW(win), "GTK on PeetOS");
    gtk_window_set_default_size(GTK_WINDOW(win), 520, 380);
    gtk_window_set_decorated(GTK_WINDOW(win), FALSE);
    g_signal_connect(win, "destroy", G_CALLBACK(gtk_main_quit), NULL);
    g_signal_connect_after(win, "draw", G_CALLBACK(on_window_draw), NULL);

    GtkWidget *box = gtk_box_new(GTK_ORIENTATION_VERTICAL, 10);
    gtk_container_set_border_width(GTK_CONTAINER(box), 18);
    gtk_container_add(GTK_CONTAINER(win), box);

    GtkWidget *title = gtk_label_new(NULL);
    gtk_label_set_markup(GTK_LABEL(title),
                         "<span size='x-large' weight='bold'>GTK 3 on PeetOS</span>\n"
                         "Wayland backend, drawn with cairo and pango into wl_shm buffers.");
    gtk_label_set_xalign(GTK_LABEL(title), 0);
    gtk_box_pack_start(GTK_BOX(box), title, FALSE, FALSE, 0);

    GtkWidget *art = gtk_drawing_area_new();
    gtk_widget_set_size_request(art, -1, 70);
    g_signal_connect(art, "draw", G_CALLBACK(draw_art), NULL);
    gtk_box_pack_start(GTK_BOX(box), art, FALSE, FALSE, 0);

    entry = gtk_entry_new();
    gtk_entry_set_placeholder_text(GTK_ENTRY(entry), "Type here");
    g_signal_connect(entry, "changed", G_CALLBACK(on_entry_changed), NULL);
    gtk_box_pack_start(GTK_BOX(box), entry, FALSE, FALSE, 0);

    progress = gtk_progress_bar_new();
    gtk_box_pack_start(GTK_BOX(box), progress, FALSE, FALSE, 0);

    GtkWidget *row = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 10);
    check = gtk_check_button_new_with_label("Button was clicked");
    gtk_box_pack_start(GTK_BOX(row), check, TRUE, TRUE, 0);
    GtkWidget *combo = gtk_combo_box_text_new();
    gtk_combo_box_text_append_text(GTK_COMBO_BOX_TEXT(combo), "Adwaita");
    gtk_combo_box_text_append_text(GTK_COMBO_BOX_TEXT(combo), "HighContrast");
    gtk_combo_box_set_active(GTK_COMBO_BOX(combo), 0);
    gtk_box_pack_start(GTK_BOX(row), combo, FALSE, FALSE, 0);
    gtk_box_pack_start(GTK_BOX(box), row, FALSE, FALSE, 0);

    button = gtk_button_new_with_label("Click me");
    gtk_style_context_add_class(gtk_widget_get_style_context(button), "suggested-action");
    g_signal_connect(button, "clicked", G_CALLBACK(on_clicked), NULL);
    gtk_box_pack_end(GTK_BOX(box), button, FALSE, FALSE, 0);

    gtk_widget_show_all(win);
    gtk_widget_grab_focus(entry);
    gtk_main();
    return 0;
}
