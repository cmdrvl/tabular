# Regenerates the temporal / decimal fixtures and their expected text.
# Independent writers on purpose (pyarrow, openpyxl) and independent expectations
# (Python's datetime and decimal modules): the Rust renderer must not be validated
# against its own arithmetic.
#   uv run --with pyarrow --with openpyxl python make_temporal_fixtures.py .
import datetime as dt, decimal, json, sys
import openpyxl, pyarrow as pa, pyarrow.parquet as pq
from openpyxl.utils.datetime import CALENDAR_MAC_1904

out = sys.argv[1]
D = decimal.Decimal
decimal.getcontext().prec = 100  # the default 28 digits cannot hold decimal(38) / decimal(50)
EPOCH = dt.datetime(1970, 1, 1)

# ---- Parquet: one row per interesting value, one column per physical/logical kind ----
dates = [dt.date(2024, 2, 29), dt.date(1969, 12, 31), None, dt.date(1, 1, 1)]
# (microsecond-exact values; the narrower units are derived exactly from these)
naive = [
    dt.datetime(2018, 11, 2, 18, 42, 9, 153000),
    dt.datetime(1969, 12, 31, 23, 59, 59, 999000),
    None,
    dt.datetime(2262, 4, 11, 23, 47, 16, 854000),
]
decs = [D("12.50"), D("-0.05"), None, D("-99999999.99")]
dec0 = [D("42"), D("-1"), None, D("99999")]
dec_big = [D("9999999999999999999999999999.9999999999"), D("-0.0000000001"), None, D("0E-10")]
dec_wide = [D("123456789012345678901234567890123456789012345.67890"), D("-1.00000"), None, D("0.00001")]
times_ms = [dt.time(0, 0, 0), dt.time(23, 59, 59, 999000), None, dt.time(12, 30, 15, 250000)]

utc = dt.timezone.utc
schema = pa.schema([
    ("id", pa.int32()),
    ("d", pa.date32()),
    ("ts_ms_utc", pa.timestamp("ms", tz="UTC")),
    ("ts_us_utc", pa.timestamp("us", tz="UTC")),
    ("ts_ns_utc", pa.timestamp("ns", tz="UTC")),
    ("ts_ms_naive", pa.timestamp("ms")),
    ("ts_us_naive", pa.timestamp("us")),
    ("ts_ns_naive", pa.timestamp("ns")),
    ("t_ms", pa.time32("ms")),
    ("t_us", pa.time64("us")),
    ("t_ns", pa.time64("ns")),
    ("dec_2", pa.decimal128(10, 2)),
    ("dec_0", pa.decimal128(5, 0)),
    ("dec_big", pa.decimal128(38, 10)),
    ("dec_wide", pa.decimal256(50, 5)),
])
cols = {
    "id": [1, 2, 3, 4],
    "d": dates,
    "ts_ms_utc": [None if v is None else v.replace(tzinfo=utc) for v in naive],
    "ts_us_utc": [None if v is None else v.replace(tzinfo=utc) for v in naive],
    "ts_ns_utc": [None if v is None else v.replace(tzinfo=utc) for v in naive],
    "ts_ms_naive": naive,
    "ts_us_naive": naive,
    "ts_ns_naive": naive,
    "t_ms": times_ms,
    "t_us": times_ms,
    "t_ns": times_ms,
    "dec_2": decs,
    "dec_0": dec0,
    "dec_big": dec_big,
    "dec_wide": dec_wide,
}
pq.write_table(pa.table(cols, schema=schema), f"{out}/temporal.parquet", compression="snappy")

def ts_text(v, digits, z):
    if v is None:
        return ""
    frac = f"{v.microsecond:06d}"
    frac = (frac + "000")[:digits] if digits > 6 else frac[:digits]
    return v.strftime("%Y-%m-%dT%H:%M:%S") + "." + frac + ("Z" if z else "")

def time_text(v, digits):
    if v is None:
        return ""
    frac = f"{v.microsecond:06d}"
    frac = (frac + "000")[:digits] if digits > 6 else frac[:digits]
    return v.strftime("%H:%M:%S") + "." + frac

def dec_text(v, scale):
    if v is None:
        return ""
    return format(v.quantize(D(1).scaleb(-scale)), "f")

expected_parquet = {
    "headers": [f.name for f in schema],
    "rows": [
        [
            str(i + 1),
            "" if dates[i] is None else dates[i].isoformat(),
            ts_text(naive[i], 3, True), ts_text(naive[i], 6, True), ts_text(naive[i], 9, True),
            ts_text(naive[i], 3, False), ts_text(naive[i], 6, False), ts_text(naive[i], 9, False),
            time_text(times_ms[i], 3), time_text(times_ms[i], 6), time_text(times_ms[i], 9),
            dec_text(decs[i], 2), dec_text(dec0[i], 0), dec_text(dec_big[i], 10), dec_text(dec_wide[i], 5),
        ]
        for i in range(4)
    ],
}
json.dump(expected_parquet, open(f"{out}/temporal.parquet.expected.json", "w"), indent=1)

# ---- XLSX (1900 system) ----
def serial_free_rows():
    return [
        (1, dt.date(2024, 2, 29), dt.datetime(2025, 10, 13, 12, 59, 2, 400000), dt.time(12, 0, 0)),
        (2, dt.date(1999, 12, 31), dt.datetime(1900, 3, 1, 0, 0, 0), dt.time(0, 0, 0)),
        (3, dt.date(2262, 4, 11), dt.datetime(2031, 7, 9, 23, 59, 59, 999000), dt.time(23, 59, 59, 500000)),
    ]

wb = openpyxl.Workbook(); ws = wb.active; ws.title = "t"
ws.append(["id", "day", "stamp", "clock"])
for r in serial_free_rows():
    ws.append(list(r))
wb.save(f"{out}/temporal.xlsx")

def xl_day(v):
    return v.isoformat()

def xl_stamp(v):
    if v.hour == v.minute == v.second == v.microsecond == 0:
        return v.date().isoformat()
    return v.strftime("%Y-%m-%dT%H:%M:%S") + f".{v.microsecond // 1000:03d}"

def xl_clock(v):
    return v.strftime("%H:%M:%S") + f".{v.microsecond // 1000:03d}"

expected_xlsx = {
    "headers": ["id", "day", "stamp", "clock"],
    "rows": [[str(r[0]), xl_day(r[1]), xl_stamp(r[2]), xl_clock(r[3])] for r in serial_free_rows()],
}
json.dump(expected_xlsx, open(f"{out}/temporal.xlsx.expected.json", "w"), indent=1)

# ---- XLSX 1904 date system (dates and datetimes with a date part) ----
wb = openpyxl.Workbook(); wb.epoch = CALENDAR_MAC_1904
ws = wb.active; ws.title = "t"; ws.append(["id", "day", "stamp"])
ws.append([1, dt.date(2024, 2, 29), dt.datetime(2025, 10, 13, 12, 59, 2, 400000)])
ws.append([2, dt.date(1904, 1, 2), dt.datetime(1999, 12, 31, 0, 0, 0)])
wb.save(f"{out}/temporal_1904.xlsx")
json.dump({"headers": ["id", "day", "stamp"], "rows": [
    ["1", "2024-02-29", "2025-10-13T12:59:02.400"],
    ["2", "1904-01-02", "1999-12-31"],
]}, open(f"{out}/temporal_1904.xlsx.expected.json", "w"), indent=1)

# 1904 workbook with a time-of-day: serial < 1 is ambiguous there, so it must refuse.
wb = openpyxl.Workbook(); wb.epoch = CALENDAR_MAC_1904
ws = wb.active; ws.title = "t"; ws.append(["id", "clock"]); ws.append([1, dt.time(12, 0, 0)])
wb.save(f"{out}/temporal_1904_time.xlsx")

# Serial 60 with a date format: Excel's nonexistent 1900-02-29. Must refuse.
wb = openpyxl.Workbook(); ws = wb.active; ws.title = "t"; ws.append(["id", "day"])
ws.append([1, 60]); ws["B2"].number_format = "yyyy-mm-dd"
wb.save(f"{out}/excel_1900_leap_bug.xlsx")

# Out-of-range serial (beyond 9999-12-31) with a date format. Must refuse.
wb = openpyxl.Workbook(); ws = wb.active; ws.title = "t"; ws.append(["id", "day"])
ws.append([1, 3000000]); ws["B2"].number_format = "yyyy-mm-dd"
wb.save(f"{out}/excel_serial_out_of_range.xlsx")

# Binary column: no canonical text exists, still refused.
pq.write_table(pa.table({"id": [1], "blob": pa.array([b"secret-bytes"], pa.binary())}),
               f"{out}/binary.parquet")

# Duration-formatted cell ([h]:mm:ss). Still refused in v1.
wb = openpyxl.Workbook(); ws = wb.active; ws.title = "t"; ws.append(["id", "elapsed"])
ws.append([1, dt.timedelta(hours=36, minutes=30)])
wb.save(f"{out}/excel_duration.xlsx")
