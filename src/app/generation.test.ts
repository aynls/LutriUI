import { expect, test } from "bun:test";
import { resultPublishFor } from "./generation";

test("the foreground submission replaces the workspace result", () => {
  expect(resultPublishFor("job-a", undefined, false)).toBeUndefined();
  expect(resultPublishFor("job-a", "job-a", false)).toBeUndefined();
});

test("a cancelled foreground job settles without a completion notice", () => {
  expect(resultPublishFor("job-a", "job-a", true)).toEqual({ quiet: true });
  expect(resultPublishFor("job-a", undefined, true)).toEqual({ quiet: true });
});

test("an older job cannot replace a newer submission of the same intent", () => {
  expect(resultPublishFor("job-a", "job-b", false)).toEqual({
    settleOnly: true,
    quiet: true,
  });
  expect(resultPublishFor("job-a", "job-b", true)).toEqual({
    settleOnly: true,
    quiet: true,
  });
});
