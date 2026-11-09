// libpeetcxxplugin.so: a C++ plugin cxx_smoke loads at run time with dlopen().

#include "peetcxx.h"

#include <cmath>

namespace {

int g_plugin_ctor_ran = 0;

struct PluginInit {
    PluginInit() { g_plugin_ctor_ran = 1; }
};

PluginInit g_plugin_init;

class Circle final : public peetcxx::Shape {
public:
    explicit Circle(double radius) : radius_(radius) {}
    const char *name() const override { return "circle"; }
    double area() const override { return M_PI * radius_ * radius_; }

private:
    double radius_;
};

} // namespace

extern "C" peetcxx::Shape *peetcxx_plugin_make_shape(double radius) {
    if (radius < 0) {
        // Unwinds out of the dlopen()ed object into the executable.
        throw peetcxx::LibraryError("negative radius", -1);
    }
    return new Circle(radius);
}

extern "C" int peetcxx_plugin_ctor_ran() { return g_plugin_ctor_ran; }
