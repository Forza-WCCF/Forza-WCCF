# -*- coding: utf-8 -*-
"""test_backup_manifest.py - the english_backup list reads entries written under the old key (check.ps1 runs it): kits
before 2026-10-12 named the original file's fingerprint after the publisher, newer ones write "original".
python source\\test_backup_manifest.py"""
import os
import sys

KIT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.join(KIT, "code"))
import english as E  # noqa: E402

old_key = bytes.fromhex("73656761").decode()                 # the name old kits wrote
for entry in ({old_key: "AB", "english": "CD"}, {"original": "AB", "english": "CD"}, {old_key: "AB"}):
    assert E.original_of(entry) == "AB", entry
print("backup list: old and new keys both read")
