// Held by the client. The agent never sees these before submitting.
import { expect, test } from "bun:test";
import { splitInvoice } from "../src/split";

const sum = (xs: number[]) => xs.reduce((a, b) => a + b, 0);

test("weighted split adds up", () => {
  expect(sum(splitInvoice(10001, [3, 2, 2]))).toBe(10001);
});

test("leftover cents go to the largest remainders, ties to the earlier payee", () => {
  expect(splitInvoice(1000, [1, 1, 1])).toEqual([334, 333, 333]);
  expect(splitInvoice(100, [1, 2])).toEqual([33, 67]);
});

test("no payee gets a negative or fractional amount", () => {
  for (const p of splitInvoice(7, [5, 1, 1, 1])) {
    expect(Number.isInteger(p) && p >= 0).toBe(true);
  }
});

test("still rejects bad input", () => {
  expect(() => splitInvoice(-1, [1])).toThrow();
  expect(() => splitInvoice(10, [])).toThrow();
});
