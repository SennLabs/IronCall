import { fireEvent, render, screen } from "@solidjs/testing-library";
import { describe, expect, it, vi } from "vitest";
import { Modal } from "./Modal";

describe("Modal", () => {
  it("Escape closes only the topmost of two stacked dialogs", () => {
    const closeMain = vi.fn();
    const closePrompt = vi.fn();
    render(() => (
      <>
        <Modal title="Main" onClose={closeMain}>
          <input aria-label="Main field" />
        </Modal>
        <Modal title="Prompt" onClose={closePrompt}>
          <input aria-label="Prompt field" />
        </Modal>
      </>
    ));
    fireEvent.keyDown(screen.getByLabelText("Prompt field"), { key: "Escape" });
    expect(closePrompt).toHaveBeenCalledTimes(1);
    expect(closeMain).not.toHaveBeenCalled();
  });
});
