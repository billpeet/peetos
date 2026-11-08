/*
 * Host-side check for the staged xkeyboard-config subset: compile a keymap
 * for every requested layout (evdev rules, pc105 model) against a given XKB
 * root and fail if any of them does not compile.
 *
 * Usage: xkb-check <xkb-root> <layout[:variant]>[,...] [options]
 */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include <xkbcommon/xkbcommon.h>

int main(int argc, char **argv) {
    if (argc < 3) {
        fprintf(stderr, "usage: xkb-check <xkb-root> <layouts> [options]\n");
        return 2;
    }
    const char *options = argc > 3 && argv[3][0] != '\0' ? argv[3] : NULL;
    struct xkb_context *ctx = xkb_context_new(XKB_CONTEXT_NO_DEFAULT_INCLUDES |
                                              XKB_CONTEXT_NO_ENVIRONMENT_NAMES);
    if (ctx == NULL || !xkb_context_include_path_append(ctx, argv[1])) {
        fprintf(stderr, "xkb-check: cannot use XKB root %s\n", argv[1]);
        return 1;
    }

    char *list = strdup(argv[2]);
    int failed = 0;
    for (char *entry = strtok(list, ","); entry != NULL; entry = strtok(NULL, ",")) {
        char *variant = strchr(entry, ':');
        if (variant != NULL) {
            *variant++ = '\0';
        }
        struct xkb_rule_names names = {
            .rules = "evdev",
            .model = "pc105",
            .layout = entry,
            .variant = variant,
            .options = options,
        };
        struct xkb_keymap *keymap = xkb_keymap_new_from_names(ctx, &names, 0);
        if (keymap == NULL) {
            fprintf(stderr, "xkb-check: FAIL layout=%s variant=%s options=%s\n", entry,
                    variant ? variant : "-", options ? options : "-");
            failed = 1;
            continue;
        }
        char *text = xkb_keymap_get_as_string(keymap, XKB_KEYMAP_FORMAT_TEXT_V1);
        printf("xkb-check: ok layout=%s variant=%s (%zu bytes)\n", entry, variant ? variant : "-",
               text ? strlen(text) : 0);
        free(text);
        xkb_keymap_unref(keymap);
    }
    free(list);
    xkb_context_unref(ctx);
    return failed;
}
