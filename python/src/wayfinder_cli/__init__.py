"""Install and run the wayfinder binary.

This package ships no Python SDK. It exists so a Python toolchain can put the
`wayfinder` command on the path with `uv tool install wayfinder-cli` or
`pip install wayfinder-cli`, and it hands every argument to the release binary.
"""

__version__ = "0.1.0"
