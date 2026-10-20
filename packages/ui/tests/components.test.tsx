// SPDX-License-Identifier: Apache-2.0
import { createRef } from "react";
import { describe, expect, it } from "@rstest/core";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { Button } from "@delino/ui/components/button";
import { Input } from "@delino/ui/components/input";
import { Label } from "@delino/ui/components/label";
import { Checkbox } from "@delino/ui/components/checkbox";
import { Switch } from "@delino/ui/components/switch";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogTitle,
  DialogTrigger,
} from "@delino/ui/components/dialog";
import { cn } from "@delino/ui/lib/utils";

describe("shared UI consumer contracts", () => {
  it("merges consumer utility overrides", () => {
    expect(cn("px-2 text-sm", false, { "px-4": true }, "text-lg")).toBe(
      "px-4 text-lg",
    );
  });

  it("composes links without nested buttons and passes refs", () => {
    const ref = createRef<HTMLAnchorElement>();
    render(
      <Button asChild variant="outline">
        <a href="/settings" ref={ref}>
          Settings
        </a>
      </Button>,
    );
    expect(screen.queryByRole("button")).toBeNull();
    expect(ref.current).toBe(screen.getByRole("link", { name: "Settings" }));
    expect(ref.current?.getAttribute("href")).toBe("/settings");
  });

  it("preserves native form semantics and accessible labels", () => {
    render(
      <form aria-label="Settings">
        <Label htmlFor="name">Name</Label>
        <Input id="name" name="name" defaultValue="Delino" />
        <Button type="submit" disabled>
          Save
        </Button>
      </form>,
    );
    expect(screen.getByLabelText("Name").tagName).toBe("INPUT");
    const form = screen.getByRole("form") as HTMLFormElement;
    expect(new FormData(form).get("name")).toBe("Delino");
    expect((screen.getByRole("button") as HTMLButtonElement).disabled).toBe(
      true,
    );
  });

  it("exposes checked state and form values for selection controls", () => {
    render(
      <form aria-label="Options">
        <Checkbox aria-label="Accept" name="accept" />
        <Switch aria-label="Notifications" name="notifications" />
      </form>,
    );
    const checkbox = screen.getByRole("checkbox", { name: "Accept" });
    const toggle = screen.getByRole("switch", { name: "Notifications" });
    fireEvent.click(checkbox);
    fireEvent.click(toggle);
    expect(checkbox.getAttribute("aria-checked")).toBe("true");
    expect(toggle.getAttribute("aria-checked")).toBe("true");
    expect(
      new FormData(screen.getByRole("form") as HTMLFormElement).get("accept"),
    ).toBe("on");
  });

  it("focuses the critical input and restores focus after Escape", async () => {
    render(
      <Dialog>
        <DialogTrigger asChild>
          <Button>Edit</Button>
        </DialogTrigger>
        <DialogContent closeLabel="닫기">
          <DialogTitle>Edit name</DialogTitle>
          <DialogDescription>Choose a display name.</DialogDescription>
          <Input aria-label="Name" autoFocus />
        </DialogContent>
      </Dialog>,
    );
    const trigger = screen.getByRole("button", { name: "Edit" });
    fireEvent.click(trigger);
    expect(
      screen.getByRole("dialog").getAttribute("aria-labelledby"),
    ).toBeTruthy();
    await waitFor(() =>
      expect(document.activeElement).toBe(screen.getByLabelText("Name")),
    );
    expect(screen.getByRole("button", { name: "닫기" })).toBeTruthy();
    fireEvent.keyDown(document.activeElement!, { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    await waitFor(() => expect(document.activeElement).toBe(trigger));
  });
});
