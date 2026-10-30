import { afterEach, describe, expect, it } from "vitest";
import { DEFAULT_PREFS, loadPrefs, savePrefs } from "./prefs";

afterEach(() => localStorage.clear());

describe("workbench preferences", () => {
  it("defaults the sidebar to the measured width", () => {
    expect(loadPrefs().sidebarWidth).toBe(400);
  });

  it("clamps stored sidebar widths to the supported drag range", () => {
    localStorage.setItem("owl-wb-prefs", JSON.stringify({ sidebarWidth: 900 }));
    expect(loadPrefs().sidebarWidth).toBe(700);
    localStorage.setItem("owl-wb-prefs", JSON.stringify({ sidebarWidth: 200 }));
    expect(loadPrefs().sidebarWidth).toBe(300);
  });

  it("persists sidebar width alongside other workbench preferences", () => {
    savePrefs({ ...DEFAULT_PREFS, sidebarWidth: 544, theme: "sand" });
    expect(loadPrefs()).toMatchObject({ sidebarWidth: 544, theme: "sand" });
  });
});
