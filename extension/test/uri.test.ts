import { describe, expect, it } from "vitest";
import { BLOB_URI, uriView } from "../src/shared/constants";

describe("uriView (blob: data availability URIs)", () => {
  const id = "ab".repeat(32);
  it("shortens blob store URIs and keeps the full one", () => {
    const v = uriView(`blob:${id}`);
    expect(v.blob).toBe(true);
    expect(v.full).toBe(`blob:${id}`);
    expect(v.value).toBe("blob abababab...ababab (chain blob store)".replace("abababab", "ababab"));
    expect(BLOB_URI.test(`blob:${id}`)).toBe(true);
  });
  it("flags malformed blob URIs and passes others through", () => {
    expect(uriView("blob:1234").value).toContain("malformed");
    expect(uriView("ipfs://x")).toEqual({ value: "ipfs://x", blob: false });
    expect(uriView("").value).toBe("none");
  });
});
