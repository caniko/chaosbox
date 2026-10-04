// Byte-exact reference table for chaosbox's sessionHash encoder.
//
// Emits JSON that the Rust unit tests embed verbatim, so the expectations are
// produced by `JSON.stringify` itself rather than transcribed by hand.

const reals = [
  0.1, 0.2, 0.30000000000000004, 0.448854, 0.5, 1.25, 1.5, 2.5,
  3.6143699999999996, 7.3637754, 30.407653999999997, 87.12209799999997,
  100.5, 123.456, 12345,
  0.0001051064, 0.00001, 0.000001, 0.0001, 0.001, 0.01,
  1e-7, 1e-9, 1e-12, 1e-21, 1e-300, 5e-324,
  1, 7, 10, 100, 1000, 10000, 100000, 1000000, 10000000,
  1e15, 1e16, 1e17, 1e19, 1e20, 1e21, 1.5e21, 2.5e21, 1e22, 1e30,
  9007199254740991, 9007199254740993,
  1.7976931348623157e308,
  -1, -0.5, -100.5, -0.0000012, -1.5e-7, -1e21, -1.7976931348623157e308,
];

// `JSON.stringify(-0)` renders as `0`, so the negative-zero input is written
// by hand as "-0": parsing it back yields `-0.0`, which `JSON.stringify`
// prints as `0`.
const realCases = reals.map((n) => [String(n), JSON.stringify(n)]);
realCases.push(["-0", "0"]);

const integers = [
  "0", "1", "-1", "7", "12345", "-12345", "1000000",
  "9007199254740991", "9007199254740992", "9007199254740993",
  "9223372036854775807", "-9223372036854775808",
  "1000000000000000000",
];
const integerCases = integers.map((s) => [s, JSON.stringify(Number(s))]);

const strings = [
  "plain",
  "a\"b",
  "a\\b",
  "line\nbreak",
  "tab\there",
  "back\bhere",
  "form\ffeed",
  "carriage\rret",
  "\u0000",
  "\u0001",
  "\u001f",
  "\u007f",
  "\u2028",
  "\u2029",
  "\u2028\u2029",
  "'single'",
  "`backtick`",
  ";semi",
  "%pct",
  "_under",
  "$dollar",
  "#hash",
  "é中😀",
  "{\"b\":1,\"a\":2}",
  "  padded  ",
  "",
  "\u0001\u0002\u0003 ctrl with space and é",
  "path/to/file.txt",
  "--flag=value",
];

const stringCases = strings.map((s) => [s, JSON.stringify(s)]);

// Fragment goldens: exactly the rows the test fixture inserts, canonicalized
// by the same `canonical()` the migration used (keys sorted at every level).
const canonical = (v) => {
  if (Array.isArray(v)) return v.map(canonical);
  if (v && typeof v === "object") {
    return Object.fromEntries(Object.keys(v).sort().map((k) => [k, canonical(v[k])]));
  }
  return v;
};

const sessionRow = {
  id: "ses_x",
  seq: 3,
  time: 1790166653727,
  cost: 3.6143699999999996,
  meta: '{"b":1,"a":2}',
  nul: null,
};

const messageRow = {
  session_id: "ses_x",
  id: "m1",
  seq: 1,
  role: "user",
  cost: 0,
  time: 1790166653727,
  text: "line\nbreak",
};

const linkRow = {
  session_id: "ses_x",
  id: "m1",
  seq: 3,
  time: 1790166653727,
  cost: 3.6143699999999996,
  meta: '{"b":1,"a":2}',
  nul: null,
};

const fragments = {
  session: JSON.stringify(canonical(sessionRow)),
  message: JSON.stringify(canonical(messageRow)),
  tuple: JSON.stringify(canonical(["session_pending", linkRow])),
};

process.stdout.write(
  JSON.stringify({ realCases, integerCases, stringCases, fragments }),
);
process.stdout.write("\n");
