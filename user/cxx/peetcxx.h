// Shared interface between cxx_smoke, libpeetcxx.so and the dlopen()ed
// libpeetcxxplugin.so. Exercises C++ ABI features that cross shared-object
// boundaries: vtables, RTTI, exceptions and static constructors.
#pragma once

#include <stdexcept>
#include <string>

namespace peetcxx {

// Thrown by libpeetcxx.so and caught in the executable: the type_info must
// match across objects for the catch clause to fire.
class LibraryError : public std::runtime_error {
public:
    LibraryError(const std::string &what, int code)
        : std::runtime_error(what), code_(code) {}
    int code() const noexcept { return code_; }

private:
    int code_;
};

class Shape {
public:
    virtual ~Shape();
    virtual const char *name() const = 0;
    virtual double area() const = 0;
};

// Defined in libpeetcxx.so.
Shape *make_square(double side);
void throw_library_error(int code);
int library_ctor_count();
std::string library_greeting(const std::string &who);

} // namespace peetcxx

// The plugin's entry point, looked up with dlsym().
extern "C" peetcxx::Shape *peetcxx_plugin_make_shape(double radius);
extern "C" int peetcxx_plugin_ctor_ran();
