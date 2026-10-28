import { beforeEach, describe, expect, it } from "vitest";
import { applyTheme, cycleTheme, setTheme, theme } from "./theme";

describe("theme", () => {
  beforeEach(() => {
    localStorage.clear();
    setTheme("system");
  });

  it("system preference removes the data-theme attribute", () => {
    const root = document.createElement("html");
    root.dataset.theme = "dark";
    applyTheme("system", root);
    expect(root.dataset.theme).toBeUndefined();
  });

  it("explicit preference sets data-theme", () => {
    const root = document.createElement("html");
    applyTheme("dark", root);
    expect(root.dataset.theme).toBe("dark");
  });

  it("setTheme persists the preference", () => {
    setTheme("light");
    expect(localStorage.getItem("ironcall.theme")).toBe("light");
    expect(document.documentElement.dataset.theme).toBe("light");
  });

  it("cycleTheme goes system → light → dark → system", () => {
    const seen = [theme()];
    for (let i = 0; i < 3; i++) {
      cycleTheme();
      seen.push(theme());
    }
    expect(seen).toEqual(["system", "light", "dark", "system"]);
  });
});
