# Regenerates every file in this directory. Independent writers on purpose
# (csv, json, pyarrow, openpyxl, msoffcrypto): the readers under test must not be
# validated against their own output.
#   uv run --with pyarrow --with openpyxl --with msoffcrypto-tool python make_fixtures.py .
import csv, datetime as dt, io, json, sys
import msoffcrypto, openpyxl, pyarrow as pa, pyarrow.parquet as pq

out = sys.argv[1]
hdr = ["id", "region", "amount", "active", "note"]
rows = [
    (1, "East", 100.5, True, "NA"),
    (2, "West", 2000.0, False, "NULL"),
    (3, " North ", 0.1, True, None),
]

def canon(v):
    if v is None: return ""
    if isinstance(v, bool): return "true" if v else "false"
    if isinstance(v, float) and v == int(v): return str(int(v))
    return str(v)

for name, delim in (("sample.csv", ","), ("sample.tsv", "\t")):
    with open(f"{out}/{name}", "w", newline="") as f:
        w = csv.writer(f, delimiter=delim, lineterminator="\n")
        w.writerow(hdr)
        for r in rows: w.writerow([canon(x) for x in r])
with open(f"{out}/sample.jsonl", "w") as f:
    for r in rows: f.write(json.dumps(dict(zip(hdr, r))) + "\n")

schema = pa.schema([("id", pa.int64()), ("region", pa.string()), ("amount", pa.float64()),
                    ("active", pa.bool_()), ("note", pa.string())])
table = pa.table({h: [r[i] for r in rows] for i, h in enumerate(hdr)}, schema=schema)
pq.write_table(table, f"{out}/sample.parquet", compression="snappy")

wb = openpyxl.Workbook(); ws = wb.active; ws.title = "loans"; ws.append(hdr)
for r in rows: ws.append(list(r))
wb.create_sheet("notes").append(["scratch"])
wb.save(f"{out}/sample.xlsx")

# Parquet: several row groups x codecs, same 5 rows.
many = [(i, f"r{i}", i * 1.5, i % 2 == 0, None if i == 3 else f"n{i}") for i in range(1, 6)]
mtable = pa.table({h: [r[i] for r in many] for i, h in enumerate(hdr)}, schema=schema)
for codec in ("zstd", "gzip", "lz4"):
    pq.write_table(mtable, f"{out}/rowgroups_{codec}.parquet", compression=codec, row_group_size=2)

# Parquet float32 (0.1f32 must render "0.1", not 0.10000000149011612).
pq.write_table(pa.table({"v": pa.array([0.1, 2.0, 100.5], pa.float32())}), f"{out}/float32.parquet")
# Parquet types v0 refuses.
pq.write_table(pa.table({"id": [1], "closed": pa.array([dt.date(2024, 5, 17)], pa.date32())}),
               f"{out}/unsupported_date.parquet")
pq.write_table(pa.table({"id": [1], "tags": pa.array([["x", "y"]], pa.list_(pa.string()))}),
               f"{out}/nested_list.parquet")

# XLSX negatives and layout cases.
wb = openpyxl.Workbook(); ws = wb.active; ws.title = "loans"; ws.append(hdr)
for r in rows: ws.append(list(r))
ws2 = wb.create_sheet("scratch"); ws2.append(["x"]); ws2.sheet_state = "hidden"
wb.save(f"{out}/hidden_sheet.xlsx")

wb = openpyxl.Workbook(); ws = wb.active; ws.title = "1"; ws.append(["a"]); ws.append(["from-name-1"])
ws = wb.create_sheet("x"); ws.append(["a"]); ws.append(["from-index-1"])
wb.save(f"{out}/digit_sheet_names.xlsx")

wb = openpyxl.Workbook(); ws = wb.active; ws.title = "off"
for c, h in enumerate(hdr): ws.cell(row=5, column=3 + c, value=h)
for i, r in enumerate(rows):
    for c, v in enumerate(r): ws.cell(row=6 + i, column=3 + c, value=v)
wb.save(f"{out}/off_origin.xlsx")

wb = openpyxl.Workbook(); ws = wb.active; ws.title = "d"
ws.append(["id", "closed"]); ws.append([1, dt.datetime(2031, 7, 9)])
wb.save(f"{out}/unsupported_datetime.xlsx")

# Encrypted workbook (OLE container around the encrypted package).
buf = io.BytesIO(); wb2 = openpyxl.Workbook(); wb2.active.append(["secret"]); wb2.save(buf); buf.seek(0)
enc = io.BytesIO(); msoffcrypto.OfficeFile(buf).encrypt("pw", enc)
open(f"{out}/encrypted.xlsx", "wb").write(enc.getvalue())
