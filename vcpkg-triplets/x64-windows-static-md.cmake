# vcpkg's x64-windows-static-md (static libraries, the shared C runtime Rust links by default), release only:
# Cargo links the release libraries in every profile, so building debug ones would only double the wait.
set(VCPKG_TARGET_ARCHITECTURE x64)
set(VCPKG_CRT_LINKAGE dynamic)
set(VCPKG_LIBRARY_LINKAGE static)
set(VCPKG_BUILD_TYPE release)
