# Fixtures for formats and workbook features the readers claim to handle:
# legacy .xls (BIFF8), .xlsm, XLSX formulas with cached results, merged cells, and
# Parquet brotli. Independent writers on purpose: xlwt (BIFF), xlsxwriter and
# openpyxl (OOXML), pyarrow (Parquet). Expected text is written here by hand from the
# values put in, never read back through this crate.
#   uv run --with pyarrow --with openpyxl --with xlwt --with xlsxwriter python make_format_fixtures.py .
import datetime as dt, json, sys, zipfile
import openpyxl, pyarrow as pa, pyarrow.parquet as pq, xlsxwriter, xlwt

out = sys.argv[1]

def expect(name, headers, rows):
    json.dump({"headers": headers, "rows": rows}, open(f"{out}/{name}.expected.json", "w"), indent=1)

# ---- legacy .xls (BIFF8) ----
wb = xlwt.Workbook()
ws = wb.add_sheet("loans")
for c, h in enumerate(["id", "name", "amount", "closed", "stamp"]):
    ws.write(0, c, h)
date_style = xlwt.easyxf(num_format_str="YYYY-MM-DD")
stamp_style = xlwt.easyxf(num_format_str="YYYY-MM-DD HH:MM:SS")
ws.write(1, 0, 1); ws.write(1, 1, "East"); ws.write(1, 2, 100.5)
ws.write(1, 3, dt.date(2024, 2, 29), date_style)
ws.write(1, 4, dt.datetime(2025, 10, 13, 12, 59, 2), stamp_style)
ws.write(2, 0, 2); ws.write(2, 1, " West "); ws.write(2, 2, 2000.0)
ws.write(2, 3, dt.date(1999, 12, 31), date_style)
ws.write(2, 4, dt.datetime(2031, 7, 9, 0, 0, 0), stamp_style)
wb.save(f"{out}/legacy.xls")
expect("legacy.xls", ["id", "name", "amount", "closed", "stamp"], [
    ["1", "East", "100.5", "2024-02-29", "2025-10-13T12:59:02.000"],
    ["2", " West ", "2000", "1999-12-31", "2031-07-09"],
])

# ---- .xlsm: an OOXML package whose main part is macro-enabled and that carries a vbaProject ----
src = f"{out}/_tmp_for_xlsm.xlsx"
wb = openpyxl.Workbook(); ws = wb.active; ws.title = "macro"
ws.append(["id", "label"]); ws.append([1, "alpha"]); ws.append([2, "beta"])
wb.save(src)
with zipfile.ZipFile(src) as zin, zipfile.ZipFile(f"{out}/macro_enabled.xlsm", "w", zipfile.ZIP_DEFLATED) as zout:
    for item in zin.infolist():
        data = zin.read(item.filename)
        if item.filename == "[Content_Types].xml":
            data = data.replace(
                b"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml",
                b"application/vnd.ms-excel.sheet.macroEnabled.main+xml",
            ).replace(
                b"</Types>",
                b'<Default Extension="bin" ContentType="application/vnd.ms-office.vbaProject"/></Types>',
            )
        zout.writestr(item, data)
    zout.writestr("xl/vbaProject.bin", b"\xd0\xcf\x11\xe0 not a real macro project")
import os
os.remove(src)
expect("macro_enabled.xlsm", ["id", "label"], [["1", "alpha"], ["2", "beta"]])

# ---- XLSX formulas: cached results written by xlsxwriter ----
wb = xlsxwriter.Workbook(f"{out}/formulas.xlsx")
ws = wb.add_worksheet("f")
ws.write_row(0, 0, ["id", "sum", "text", "flag"])
ws.write(1, 0, 1)
ws.write_formula(1, 1, "=1+2", None, 3)
ws.write_formula(1, 2, '="a"&"b"', None, "ab")
ws.write_formula(1, 3, "=TRUE()", None, True)
ws.write(2, 0, 2)
ws.write_formula(2, 1, "=A3*10", None, 20)
ws.write_formula(2, 2, '=UPPER("x")', None, "X")
ws.write_formula(2, 3, "=FALSE()", None, False)
wb.close()
expect("formulas.xlsx", ["id", "sum", "text", "flag"], [
    ["1", "3", "ab", "true"],
    ["2", "20", "X", "false"],
])

# Formula that cached an error value: error cells are refused.
wb = xlsxwriter.Workbook(f"{out}/formula_error.xlsx")
ws = wb.add_worksheet("f")
ws.write_row(0, 0, ["id", "ratio"])
ws.write(1, 0, 1)
ws.write_formula(1, 1, "=1/0", None, "#DIV/0!")
wb.close()

# ---- merged cells: the value lives in the top-left cell, the rest read as empty ----
wb = openpyxl.Workbook(); ws = wb.active; ws.title = "m"
ws.append(["region", "q1", "q2"])
ws.append(["East", 1, 2])
ws.append(["East", 3, 4])
ws.merge_cells("A2:A3")   # "East" survives in A2 only
ws.append(["West", 5, 6])
ws.merge_cells("B4:C4")   # 5 survives in B4 only
wb.save(f"{out}/merged_cells.xlsx")
expect("merged_cells.xlsx", ["region", "q1", "q2"], [
    ["East", "1", "2"],
    ["", "3", "4"],
    ["West", "5", ""],
])

# ---- Parquet brotli (same five rows as sample.parquet) ----
schema = pa.schema([("id", pa.int64()), ("region", pa.string()), ("amount", pa.float64()),
                    ("active", pa.bool_()), ("note", pa.string())])
rows = [(1, "East", 100.5, True, "NA"), (2, "West", 2000.0, False, "NULL"), (3, " North ", 0.1, True, None)]
table = pa.table({h: [r[i] for r in rows] for i, h in enumerate(schema.names)}, schema=schema)
pq.write_table(table, f"{out}/brotli.parquet", compression="brotli")
