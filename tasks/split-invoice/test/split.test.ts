import { expect, test } from "bun:test";
import { splitInvoice } from "../src/split";

test("even split", () => {
  expect(splitInvoice(900, [1, 1, 1])).toEqual([300, 300, 300]);
});

test("parts add up to the total", () => {
  const parts = splitInvoice(1000, [1, 1, 1]);
  expect(parts.reduce((a, b) => a + b, 0)).toBe(1000);
});
