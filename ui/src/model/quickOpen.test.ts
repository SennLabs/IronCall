import { describe, expect, it } from "vitest";
import { type QuickOpenEntry, rankEntries } from "./quickOpen";

const e = (name: string, where = "", method = "GET", url = ""): QuickOpenEntry => ({
  id: name,
  name,
  where,
  method,
  url,
  scratch: false,
});

const ENTRIES = [
  e("List users", "Users", "GET", "{{baseUrl}}/users"),
  e("Create user", "Users", "POST", "{{baseUrl}}/users"),
  e("Health", "", "GET", "/health"),
  e("User settings", "Admin", "GET", "/settings"),
];

const names = (q: string) => rankEntries(ENTRIES, q).map((x) => x.name);

describe("quick open ranking", () => {
  it("needs every word somewhere and ranks name matches first", () => {
    expect(names("")).toHaveLength(4);
    expect(names("user")[0]).toBe("User settings");
    expect(names("users post")).toEqual(["Create user"]);
    expect(names("health")).toEqual(["Health"]);
    expect(names("admin")).toEqual(["User settings"]);
    expect(names("zzz")).toEqual([]);
  });

  it("prefers word starts over substrings, then shorter names", () => {
    expect(names("set")).toEqual(["User settings"]);
    expect(names("u")[0]).toBe("User settings");
    expect(rankEntries([e("Users list"), e("Users")], "users").map((x) => x.name)).toEqual([
      "Users",
      "Users list",
    ]);
  });
});
