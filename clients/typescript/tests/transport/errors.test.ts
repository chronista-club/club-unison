import { describe, expect, it } from "vitest";
import {
  UnisonTransportError,
  WebTransportUnsupportedError,
} from "../../src/transport/errors.js";

describe("WebTransportUnsupportedError", () => {
  it("is an Error and a UnisonTransportError", () => {
    const err = new WebTransportUnsupportedError();
    expect(err).toBeInstanceOf(Error);
    expect(err).toBeInstanceOf(UnisonTransportError);
  });

  it("carries the correct name and a message", () => {
    const err = new WebTransportUnsupportedError();
    expect(err.name).toBe("WebTransportUnsupportedError");
    expect(err.message).toMatch(/WebTransport/);
  });

  it("is categorized as a transport error", () => {
    const err = new WebTransportUnsupportedError();
    expect(err.category).toBe("transport");
  });
});
