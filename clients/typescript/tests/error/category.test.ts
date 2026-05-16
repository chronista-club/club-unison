import { describe, expect, it } from "vitest";
import { ERROR_CATEGORIES, type ErrorCategory } from "../../src/error/category.js";

describe("ErrorCategory", () => {
  it("exposes the 4 categories matching the Rust enum", () => {
    expect(ERROR_CATEGORIES).toEqual(["transport", "protocol", "application", "resource"]);
  });

  it("accepts each category as a valid ErrorCategory value", () => {
    for (const c of ERROR_CATEGORIES) {
      const category: ErrorCategory = c;
      expect(typeof category).toBe("string");
    }
  });
});
