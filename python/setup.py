"""Build a platform wheel that carries the released binary.

The package holds a native executable, so a pure-Python wheel would install on
machines whose binary it does not contain. `has_ext_modules` and the `bdist_wheel`
override below keep the wheel tagged for the platform it was built for while
leaving the tag's Python side as `py3`, because nothing here depends on a CPython
ABI.
"""

import platform
import sysconfig

from setuptools import Distribution, setup
from setuptools.command.bdist_wheel import bdist_wheel


def platform_tag() -> str:
    """Tag for the machine this wheel is built on.

    setuptools asks for a universal2 macOS wheel by default; the binary inside is
    the architecture the release was built for, so the tag must say which one.
    """
    tag = sysconfig.get_platform().replace("-", "_").replace(".", "_")
    machine = platform.machine().replace("-", "_")
    if "universal2" in tag or tag.endswith("_universal"):
        tag = f"{tag.rsplit('_', 1)[0]}_{machine}"
    # arm64 macOS did not exist before 11.0; a universal2 interpreter reports a
    # 10.x floor, and pip refuses an arm64 wheel tagged below 11.
    parts = tag.split("_")
    if tag.startswith("macosx_") and parts[-1] == "arm64" and len(parts) >= 4:
        try:
            minor = int(parts[2])
            if int(parts[1]) < 11 and minor >= 0 and int(parts[1]) == 10:
                tag = "_".join(["macosx", "11", "0", "arm64"])
        except ValueError:
            pass
    return tag


class PlatformDistribution(Distribution):
    """A distribution that installs a native binary."""

    def has_ext_modules(self):
        return True


class PlatformWheel(bdist_wheel):
    """Tag the wheel for this machine, on any Python 3."""

    def finalize_options(self):
        super().finalize_options()
        self.root_is_pure = False

    def get_tag(self):
        return ("py3", "none", platform_tag())


setup(distclass=PlatformDistribution, cmdclass={"bdist_wheel": PlatformWheel})
