#!/usr/bin/env python3
"""Import pinned, offline Hunspell dictionaries and XKB keyboard tables.

Requires libxkbcommon 1.13.2 and xkeyboard-config 2.48 at generation time only.
Pass a checkout of wooorm/dictionaries at the revision below. Runtime builds
consume the checked-in tables and never need XKB or a network connection.
"""

import argparse
import ctypes as c
import hashlib
import json
from pathlib import Path
import shutil
import subprocess

REVISION = "8cfea406b505e4d7df52d5a19bce525df98c54ab"
ROOT = Path(__file__).resolve().parents[2]
ASSETS = ROOT / "crates/fixkey-core/assets"

# Languages that share a national keyboard use that keyboard explicitly. The
# selected layout name is visible in Settings; a language is not a new layout.
LAYOUTS = {
    "bg": "bg", "br": "fr:bre", "ca": "es:cat", "ca-valencia": "es:cat",
    "cs": "cz", "cy": "gb:extd", "da": "dk", "de": "de", "de-AT": "at",
    "de-CH": "ch", "el": "gr", "el-polyton": "gr:polytonic",
    "en-AU": "us", "en-CA": "us", "en-GB": "gb", "en-ZA": "us",
    "eo": "epo", "es": "es", "et": "ee", "eu": "es", "fa": "ir",
    "fo": "fo", "fr": "fr", "fur": "it", "fy": "nl", "ga": "ie",
    "gd": "gb:extd", "gl": "es", "he": "il", "hr": "hr", "hu": "hu",
    "hy": "am", "hyw": "am:western", "ia": "us:intl", "ie": "us:intl",
    "is": "is", "it": "it", "ka": "ge", "la": "us:intl", "lb": "ch:fr",
    "lt": "lt", "ltg": "lv:modern", "lv": "lv", "mk": "mk", "mn": "mn",
    "nb": "no", "nds": "de", "ne": "np", "nl": "nl", "nn": "no",
    "oc": "fr:oci", "pl": "pl", "pt": "br", "pt-PT": "pt", "ro": "ro",
    "ru": "ru", "rw": "us:intl", "sk": "sk", "sl": "si", "sr": "rs",
    "sr-Latn": "rs:latin", "sv": "se", "sv-FI": "fi", "tk": "tm",
    "tlh-Latn": "us", "tr": "tr", "uk": "ua", "vi": "vn",
}

# These pinned upstream files contain malformed flags or affix directives.
# Keep their original bytes and notices. Listed surfaces can still be proved;
# unknown forms are unavailable, never negative dictionary evidence. Remove
# each exception after its upstream source parses and full-word fixtures pass.
LISTED_ONLY = {"br", "da", "gl", "hy", "ia", "la", "mn", "ne", "tr"}

LICENSE_TEXTS = {
    "GPL-2.0.txt": ("edaef632cbb643e4e7a221717a6c441a4c1a7c918e6e4d56debc3d8739b233f6", "https://www.gnu.org/licenses/old-licenses/gpl-2.0.txt"),
    "LGPL-2.1.txt": ("5749785c8bdefafcb5d798270ed0a967036fe2ca63dcedade1627565dfef81d2", "https://github.com/spdx/license-list-data/blob/31ba1a50e5397e00a304dbadc76531740e89ee48/text/LGPL-2.1-only.txt"),
}


def verify_source(source):
    """Reject an unpinned or modified checkout before reading or writing assets."""
    source = source.resolve()

    def git(*arguments):
        try:
            return subprocess.run(
                ["git", "-C", str(source), *arguments],
                check=True, capture_output=True, text=True,
            ).stdout.strip()
        except subprocess.CalledProcessError as error:
            raise RuntimeError("Dictionary source must be a readable Git checkout") from error

    if Path(git("rev-parse", "--show-toplevel")).resolve() != source:
        raise RuntimeError("Dictionary source must be the Git checkout root")
    revision = git("rev-parse", "HEAD")
    if revision != REVISION:
        raise RuntimeError(f"Dictionary source revision must be {REVISION}; found {revision}")
    if git("status", "--porcelain=v1", "--untracked-files=all"):
        raise RuntimeError("Dictionary source must have no modified or untracked files")
    return revision


def bind(lib, name, result, *args):
    fn = getattr(lib, "xkb_" + name)
    fn.restype, fn.argtypes = result, args
    return fn


class Names(c.Structure):
    _fields_ = [(name, c.c_char_p) for name in ["rules", "model", "layout", "variant", "options"]]


def keyboard_tables(library, include, compose):
    lib = c.CDLL(library)
    ptr, uint = c.c_void_p, c.c_uint32
    # Exclude default include paths and RMLVO environment overrides. Only the
    # explicitly supplied pinned keyboard tree may influence these tables.
    context = bind(lib, "context_new", ptr, c.c_int)(3)
    bind(lib, "context_include_path_append", c.c_int, ptr, c.c_char_p)(context, str(include).encode())
    make = bind(lib, "keymap_new_from_names", ptr, ptr, c.POINTER(Names), c.c_int)
    key = bind(lib, "keymap_key_by_name", uint, ptr, c.c_char_p)
    symbols = bind(lib, "keymap_key_get_syms_by_level", c.c_int, ptr, uint, uint, uint, c.POINTER(c.POINTER(uint)))
    utf32 = bind(lib, "keysym_to_utf32", uint, uint)
    compose_bytes = compose.read_bytes()
    table = bind(lib, "compose_table_new_from_buffer", ptr, ptr, c.c_char_p, c.c_size_t, c.c_char_p, c.c_int, c.c_int)(context, compose_bytes, len(compose_bytes), b"en_US.UTF-8", 1, 0)
    if not table:
        raise RuntimeError("X11 en_US.UTF-8 Compose data is required")
    state = bind(lib, "compose_state_new", ptr, ptr, c.c_int)(table, 0)
    feed = bind(lib, "compose_state_feed", c.c_int, ptr, uint)
    reset = bind(lib, "compose_state_reset", None, ptr)
    status = bind(lib, "compose_state_get_status", c.c_int, ptr)
    result = bind(lib, "compose_state_get_utf8", c.c_int, ptr, c.c_char_p, c.c_size_t)
    rows = [("AE", "`1234567890-=", "~!@#$%^&*()_+", 0),
            ("AD", "qwertyuiop[]", "QWERTYUIOP{}", 1),
            ("AC", "asdfghjkl;'", 'ASDFGHJKL:"', 1),
            ("AB", "zxcvbnm,./", "ZXCVBNM<>?", 1)]
    positions = [("TLDE" if prefix == "AE" and i == 0 else f"{prefix}{i + start:02}", plain, shifted)
                 for prefix, lower, upper, start in rows
                 for i, (plain, shifted) in enumerate(zip(lower, upper))]
    positions += [("BKSL", "\\", "|"), ("LSGT", "\ue080", "\ue081"), ("SPCE", " ", " ")]
    tables = {}
    for identifier in sorted(set(LAYOUTS.values()) | {"latam"}):
        layout, _, variant = identifier.partition(":")
        names = Names(b"evdev", b"pc105", layout.encode(), variant.encode(), b"")
        compiled = make(context, c.byref(names), 0)
        if not compiled:
            raise RuntimeError(f"Cannot compile XKB layout {identifier}")
        maps, syms = {}, {}
        for name, lower, upper in positions:
            for level in range(4):
                physical = upper if level % 2 else lower
                if level >= 2:
                    physical = chr(ord(physical) + (0x100 if ord(physical) >= 0xE000 else 0xE000))
                values = c.POINTER(uint)()
                count = symbols(compiled, key(compiled, name.encode()), 0, level, c.byref(values))
                if count != 1:
                    continue
                sym = values[0]
                cp = utf32(sym)
                text = chr(cp) if cp >= 32 else (chr(0xF0000 + sym) if 0xFE50 <= sym <= 0xFE93 else None)
                if text:
                    maps[physical], syms[text] = text, sym
        transforms = {}
        dead = [text for text in syms if ord(text) >= 0xF0000]
        for first in dead:
            for second, sym in syms.items():
                reset(state)
                feed(state, syms[first])
                feed(state, sym)
                if status(state) == 2:
                    buffer = c.create_string_buffer(64)
                    result(state, buffer, len(buffer))
                    transforms[first + second] = buffer.value.decode()
        tables[identifier] = {"keys": maps, "transforms": transforms, "dead": dead}
        bind(lib, "keymap_unref", None, ptr)(compiled)
    return tables


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path)
    parser.add_argument("--library", required=True)
    parser.add_argument("--xkb", type=Path, required=True)
    parser.add_argument("--xkb-license", type=Path, required=True)
    parser.add_argument("--xkb-authors", type=Path, required=True)
    parser.add_argument("--compose", type=Path, required=True)
    parser.add_argument("--compose-license", type=Path, required=True)
    args = parser.parse_args()
    revision = verify_source(args.source)
    tables = keyboard_tables(args.library, args.xkb, args.compose)
    catalog, files = [], {}
    source = args.source / "dictionaries"
    for directory in sorted(source.iterdir()):
        code = directory.name
        if code in ("en", "ko", "tlh") or not (directory / "index.aff").exists():
            continue
        layout = "latam" if code.startswith("es-") else LAYOUTS[code]
        package = json.loads((directory / "package.json").read_text())
        catalog.append({"id": f"{code.lower()}-xkb-{layout.replace(':', '-')}",
                        "locale": code, "layout": layout, "dictionary": code,
                        "name": package["description"].removesuffix(" spelling dictionary"),
                        "validation": "listed-words" if code in LISTED_ONLY else "hunspell",
                        "license": package["license"]})
        target = ASSETS / "languages" / code
        target.mkdir(parents=True, exist_ok=True)
        for name in ("index.aff", "index.dic", "license", "readme.md", "package.json"):
            if (directory / name).exists():
                shutil.copyfile(directory / name, target / name)
                files[str((target / name).relative_to(ASSETS))] = hashlib.sha256((target / name).read_bytes()).hexdigest()
            elif name != "license" or code not in ("et", "eu"):
                raise RuntimeError(f"Missing source attribution or data: {code}/{name}")
    output = ASSETS / "languages/catalog.json"
    output.write_text(json.dumps({"languages": catalog, "layouts": tables}, ensure_ascii=False, indent=2) + "\n")
    variants = ["".join(part.title() for part in item["dictionary"].split("-")) for item in catalog]
    if len(catalog) + 2 > 128:
        raise RuntimeError("Expand LanguageSet before adding more than 128 languages")
    generated = "//! Generated by scripts/fixkey/import-languages.py. Do not edit by hand.\n\n"
    generated += "/// A pinned dictionary and its canonical keyboard.\n#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]\n#[repr(u8)]\npub enum AdditionalLanguage {\n"
    generated += "".join(f"    /// {item['name']} ({item['layout']}).\n    {variant},\n" for item, variant in zip(catalog, variants))
    generated += "}\n\nimpl crate::Language {\n    /// Every supported dictionary/layout pair in stable order.\n"
    generated += f"    pub const ALL: [Self; {len(catalog) + 2}] = [Self::KoDubeolsik, Self::EnUsQwerty,\n"
    generated += "".join(f"        Self::Additional(AdditionalLanguage::{variant}),\n" for variant in variants)
    generated += "    ];\n}\n"
    (ROOT / "crates/fixkey-core/src/languages.rs").write_text(generated)
    files["languages/catalog.json"] = hashlib.sha256(output.read_bytes()).hexdigest()
    for name, source_file in (("COPYING", args.xkb_license), ("AUTHORS", args.xkb_authors)):
        shutil.copyfile(source_file, ASSETS / "notices" / f"xkeyboard-config-{name}")
        files[f"notices/xkeyboard-config-{name}"] = hashlib.sha256((ASSETS / "notices" / f"xkeyboard-config-{name}").read_bytes()).hexdigest()
    shutil.copyfile(args.compose_license, ASSETS / "notices/libX11-COPYING")
    files["notices/libX11-COPYING"] = hashlib.sha256(args.compose_license.read_bytes()).hexdigest()
    shutil.copyfile(args.source / "license", ASSETS / "notices/dictionaries-MIT")
    files["notices/dictionaries-MIT"] = hashlib.sha256((args.source / "license").read_bytes()).hexdigest()
    for name, (checksum, _) in LICENSE_TEXTS.items():
        data = (ASSETS / "notices" / name).read_bytes()
        if hashlib.sha256(data).hexdigest() != checksum:
            raise RuntimeError(f"Supplemental license checksum mismatch: {name}")
        files[f"notices/{name}"] = checksum
    manifest = json.loads((ASSETS / "manifest.json").read_text())
    manifest["languages"] = {"repository": "https://github.com/wooorm/dictionaries", "revision": revision,
                             "keyboards": "xkeyboard-config 2.48", "compiler": "libxkbcommon 1.13.2",
                             "keyboardSource": "https://xorg.freedesktop.org/archive/individual/data/xkeyboard-config/xkeyboard-config-2.48.tar.xz",
                             "keyboardSourceSha256": "b77041324f0109f77161ee43743fe04baa485866af8460d31e476ad3f7648fd5",
                             "compose": "libX11 1.8.13 en_US.UTF-8/Compose",
                             "composeSha256": hashlib.sha256(args.compose.read_bytes()).hexdigest(),
                             "listedWordsOnly": sorted(LISTED_ONLY),
                             "packageNotice": "notices/dictionaries-MIT",
                             "supplementalLicenses": {"et": ["notices/LGPL-2.1.txt", "notices/GPL-2.0.txt"], "eu": ["notices/GPL-2.0.txt"]},
                             "licenseTextSources": {f"notices/{name}": url for name, (_, url) in LICENSE_TEXTS.items()},
                             "command": "python3 scripts/fixkey/import-languages.py <pinned-checkout> --library <libxkbcommon> --xkb <xkb-data> --xkb-license <xkeyboard-config-COPYING> --xkb-authors <xkeyboard-config-AUTHORS> --compose <libX11-Compose> --compose-license <libX11-COPYING>",
                             "excluded": {"tlh": "The dictionary uses private-use pIqaD characters with no matching XKB layout."}}
    manifest["files"].update(files)
    (ASSETS / "manifest.json").write_text(json.dumps(manifest, ensure_ascii=False, indent=2) + "\n")
    attributes = ROOT / ".gitattributes"
    existing = attributes.read_text()
    for file in sorted((ASSETS / "languages").rglob("*")):
        if file.is_file() and file.stat().st_size >= 512 * 1024:
            line = f"{file.relative_to(ROOT)} filter=lfs diff=lfs merge=lfs -text\n"
            if line not in existing:
                existing += line
    attributes.write_text(existing)
    print(json.dumps({"languages": len(catalog), "layouts": len(tables), "result": "imported"}))


if __name__ == "__main__":
    main()
