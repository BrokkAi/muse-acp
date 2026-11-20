#!/usr/bin/env python3
"""Start the fake `muse serve` host defined in `fake_serve.py`.

Python compiles a script it is asked to run on every launch, but it caches the
bytecode of a module it imports. The integration suite starts the fake host
several hundred times, so the tests run this small script and it imports the
host. Every launch after the first then skips compiling `fake_serve.py`.

Scenarios and protocol behavior all live in `fake_serve.py`.
"""
import os
import sys

if __name__ == "__main__":
    sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
    import fake_serve

    fake_serve.main()
