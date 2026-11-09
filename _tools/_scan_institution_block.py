"""Dump a single institution asset Values block for structure."""
import re
from pathlib import Path

xml = Path(__file__).resolve().parent.parent.joinpath("_gamedata", "assets.xml").read_text(
    encoding="utf-8", errors="replace"
)
# Institution Roman Fire GUID 30754
m = re.search(
    r"<GUID>30754</GUID>(.*?)</Asset>",
    xml,
    re.S,
)
# Actually GUID may come after Name; find Asset containing that GUID
for m in re.finditer(r"<Asset>(.*?)</Asset>", xml, re.S):
    chunk = m.group(1)
    if "<GUID>30754</GUID>" not in chunk:
        continue
    if "Institution Roman Fire" not in chunk:
        continue
    # print Values section condensed
    vals = re.search(r"<Values>(.*?)</Values>", chunk, re.S)
    text = vals.group(1) if vals else chunk
    # collapse whitespace for readability somewhat
    print(text[:6000])
    print("\n--- len", len(text))
    break

