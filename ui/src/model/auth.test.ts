import { describe, expect, it } from "vitest";
import type { AuthDraft } from "../ipc/networkTypes";
import { describeAuth, looksLikeLiteralSecret, patchAuth, withAuthType } from "./auth";

describe("auth helpers", () => {
  it("switching type keeps unknown keys and drops old fields", () => {
    const bearer = { type: "bearer", token: "{{t}}", futureField: 7 } as AuthDraft;
    const basic = withAuthType(bearer, "basic");
    expect(basic).toEqual({ type: "basic", username: "", password: "", futureField: 7 });
    const back = withAuthType(basic, "apiKey");
    expect(back).toMatchObject({ type: "apiKey", in: "header", futureField: 7 });
    expect(back).not.toHaveProperty("username");
  });

  it("same type is a no-op", () => {
    const a = { type: "bearer", token: "x" } as AuthDraft;
    expect(withAuthType(a, "bearer")).toBe(a);
  });

  it("missing auth means inherit", () => {
    expect(withAuthType(undefined, "inherit")).toEqual({ type: "inherit" });
  });

  it("patch spreads", () => {
    const a = { type: "basic", username: "a", extra: true } as AuthDraft;
    expect(patchAuth(a, { password: "p" })).toEqual({
      type: "basic",
      username: "a",
      password: "p",
      extra: true,
    });
  });

  it("describes auths", () => {
    expect(describeAuth({ type: "bearer", token: "{{token}}" })).toBe("Bearer {{token}}");
    expect(describeAuth({ type: "apiKey", key: "X-Key", in: "query" })).toBe(
      "API key X-Key (query)",
    );
    expect(describeAuth({ type: "basic", username: "ada" })).toBe("Basic ada");
  });

  it("spots literal secrets but not variables or short words", () => {
    expect(looksLikeLiteralSecret("eyJhbGciOiJIUzI1NiJ9.e30.abc123")).toBe(true);
    expect(looksLikeLiteralSecret("sk_live_51Hx0aB2cD3eF4")).toBe(true);
    expect(looksLikeLiteralSecret("{{token}}")).toBe(false);
    expect(looksLikeLiteralSecret("Bearer {{token}}")).toBe(false);
    expect(looksLikeLiteralSecret("hunter2")).toBe(false);
    expect(looksLikeLiteralSecret("averylongpasswordwithoutdigits")).toBe(false);
  });
});
