// SPDX-License-Identifier: Apache-2.0
import { afterEach, describe, expect, test } from "@rstest/core";
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
} from "@testing-library/react";
import { Button, ButtonSize, ButtonVariant } from "./button.tsx";
import {
  Dialog,
  DialogActions,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogTitle,
  DialogTrigger,
} from "./dialog.tsx";
import { Switch } from "./switch.tsx";
import { TextField } from "./text-field.tsx";

afterEach(() => {
  cleanup();
});

describe("Button", () => {
  test("renders a native button with the variant and size", () => {
    render(
      <Button variant={ButtonVariant.Danger} size={ButtonSize.Large}>
        Delete
      </Button>,
    );

    const button = screen.getByRole("button", { name: "Delete" });
    expect(button.tagName).toBe("BUTTON");
    expect(button.getAttribute("type")).toBe("button");
    expect(button.getAttribute("data-variant")).toBe("danger");
    expect(button.getAttribute("data-size")).toBe("large");
    expect(button.className).toBe("dui-button");
  });

  test("defaults to the primary variant and medium size", () => {
    render(<Button>Save</Button>);

    const button = screen.getByRole("button", { name: "Save" });
    expect(button.getAttribute("data-variant")).toBe("primary");
    expect(button.getAttribute("data-size")).toBe("medium");
    expect(button.hasAttribute("data-full-width")).toBe(false);
  });

  test("appends a caller class name to the package class", () => {
    render(
      <Button className="extra" fullWidth>
        Save
      </Button>,
    );

    const button = screen.getByRole("button", { name: "Save" });
    expect(button.className).toBe("dui-button extra");
    expect(button.hasAttribute("data-full-width")).toBe(true);
  });

  test("stays pending until the click handler's Promise settles", async () => {
    let finish: () => void = () => {};
    let presses = 0;
    render(
      <Button
        onClick={() => {
          presses += 1;
          return new Promise<void>((resolve) => {
            finish = resolve;
          });
        }}
      >
        Save
      </Button>,
    );

    const button = screen.getByRole("button", { name: "Save" });
    fireEvent.click(button);
    fireEvent.click(button);
    expect(presses).toBe(1);
    expect(button.hasAttribute("data-pending")).toBe(true);
    expect(button.getAttribute("aria-busy")).toBe("true");

    await act(async () => {
      finish();
    });
    expect(button.hasAttribute("data-pending")).toBe(false);
  });
});

describe("TextField", () => {
  test("connects the label and description to the input", () => {
    render(<TextField label="Email" description="We never share it." />);

    const input = screen.getByRole("textbox", { name: "Email" });
    const describedBy = input.getAttribute("aria-describedby") ?? "";
    const description = screen.getByText("We never share it.");
    expect(describedBy.split(" ")).toContain(description.id);
  });

  test("marks the field invalid and shows the error message", () => {
    render(<TextField label="Email" error="Enter a valid email." />);

    const input = screen.getByRole("textbox", { name: "Email" });
    expect(input.getAttribute("aria-invalid")).toBe("true");
    expect(screen.getByText("Enter a valid email.")).toBeTruthy();
  });

  test("reports typed values", () => {
    const values: string[] = [];
    render(
      <TextField label="Name" onValueChange={(value) => values.push(value)} />,
    );

    fireEvent.input(screen.getByRole("textbox", { name: "Name" }), {
      target: { value: "Ada" },
    });
    expect(values.at(-1)).toBe("Ada");
  });
});

describe("Switch", () => {
  test("toggles when pressed and names itself with the label", () => {
    const changes: boolean[] = [];
    render(
      <Switch
        label="Email notifications"
        onCheckedChange={(checked) => changes.push(checked)}
      />,
    );

    const control = screen.getByRole("switch", { name: "Email notifications" });
    expect(control.getAttribute("aria-checked")).toBe("false");

    fireEvent.click(control);
    expect(control.getAttribute("aria-checked")).toBe("true");
    expect(changes).toEqual([true]);
  });
});

describe("Dialog", () => {
  function Example() {
    return (
      <Dialog>
        <DialogTrigger>Delete account</DialogTrigger>
        <DialogContent>
          <DialogTitle>Delete your account?</DialogTitle>
          <DialogDescription>This cannot be undone.</DialogDescription>
          <DialogActions>
            <DialogClose>Cancel</DialogClose>
          </DialogActions>
        </DialogContent>
      </Dialog>
    );
  }

  test("opens from its trigger and is named by its title", async () => {
    render(<Example />);
    expect(screen.queryByRole("dialog")).toBeNull();

    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Delete account" }));
    });

    const dialog = screen.getByRole("dialog", { name: "Delete your account?" });
    expect(dialog.className).toBe("dui-dialog-content");
  });

  test("closes with the Escape key", async () => {
    render(<Example />);
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Delete account" }));
    });
    const dialog = screen.getByRole("dialog");

    await act(async () => {
      fireEvent.keyDown(dialog, { key: "Escape" });
    });
    expect(dialog.hasAttribute("data-closed")).toBe(true);
  });

  test("closes from a close button", async () => {
    render(<Example />);
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Delete account" }));
    });
    const dialog = screen.getByRole("dialog");

    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    });
    expect(dialog.hasAttribute("data-closed")).toBe(true);
  });
});
