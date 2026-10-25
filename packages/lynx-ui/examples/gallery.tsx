// SPDX-License-Identifier: Apache-2.0
import { root, useRef, useState } from "@lynx-js/react";
import {
  Alert,
  Badge,
  Button,
  Card,
  Checkbox,
  Dialog,
  Direction,
  Field,
  Input,
  Progress,
  Separator,
  Stack,
  Switch,
  Tabs,
  Text,
  TextVariant,
  Theme,
  Tone,
  UIProvider,
  Variant,
  type FocusHandle,
} from "../src";
import "../src/styles.css";

export function Gallery() {
  const [dark, setDark] = useState(false);
  const [name, setName] = useState("");
  const [accepted, setAccepted] = useState(false);
  const [notifications, setNotifications] = useState(true);
  const [tab, setTab] = useState("overview");
  const [open, setOpen] = useState(false);
  const [saved, setSaved] = useState(false);
  const trigger = useRef<FocusHandle>(null);
  const input = useRef<FocusHandle>(null);
  return (
    <UIProvider theme={dark ? Theme.Dark : Theme.Light}>
      <scroll-view
        scroll-orientation="vertical"
        style={{ height: "100vh", width: "100%" }}
      >
        <Stack
          gap={24}
          style={{ padding: "24px", maxWidth: "960px", width: "100%" }}
        >
          <Text variant={TextVariant.Title}>Lynx UI</Text>
          <Text muted>Web / Mobile / Desktop</Text>
          <Switch
            label="Dark theme"
            checked={dark}
            onCheckedChange={setDark}
            checkedLabel="On"
            uncheckedLabel="Off"
          />
          <Card>
            <Text variant={TextVariant.Title}>Buttons</Text>
            <Stack direction={Direction.Row} wrap gap={8}>
              <Button onPress={() => setSaved(true)}>Continue</Button>
              <Button variant={Variant.Secondary}>Secondary</Button>
              <Button variant={Variant.Outline}>Cancel</Button>
              <Button
                variant={Variant.Danger}
                focusRef={trigger}
                onPress={() => setOpen(true)}
              >
                Discard changes
              </Button>
              <Button disabled>Unavailable</Button>
              <Button loading loadingLabel="Saving…">
                Save
              </Button>
            </Stack>
            {saved ? <Badge tone={Tone.Success}>Saved</Badge> : null}
          </Card>
          <Card>
            <Text variant={TextVariant.Title}>Preferences</Text>
            <Text muted>Manage your settings</Text>
            <Badge tone={Tone.Success}>Ready</Badge>
            <Separator />
            <Field
              label="Name"
              error={name.length === 0 ? "Name is required" : undefined}
            >
              <Input
                value={name}
                onValueChange={setName}
                accessibilityLabel="Name"
                placeholder="Enter a name"
                invalid={!name}
              />
            </Field>
            <Checkbox
              checked={accepted}
              onCheckedChange={setAccepted}
              label="Accept terms"
              checkedLabel="Checked"
              uncheckedLabel="Unchecked"
            />
            <Switch
              checked={notifications}
              onCheckedChange={setNotifications}
              label="Notifications"
              checkedLabel="On"
              uncheckedLabel="Off"
            />
          </Card>
          <Tabs
            label="Preferences sections"
            value={tab}
            onValueChange={setTab}
            items={[
              {
                value: "overview",
                label: "Overview",
                content: <Text>Shared components for Lynx applications.</Text>,
              },
              {
                value: "unavailable",
                label: "Unavailable",
                disabled: true,
                content: null,
              },
              {
                value: "details",
                label: "Details",
                content: <Text>Use touch or keyboard to change settings.</Text>,
              },
            ]}
          />
          <Alert title="Could not save">
            <Text>Something went wrong. Please try again.</Text>
            <Button variant={Variant.Outline} onPress={() => setSaved(true)}>
              Try again
            </Button>
          </Alert>
          <Text>Progress · 60%</Text>
          <Progress value={60} label="Progress" />
        </Stack>
      </scroll-view>
      <Dialog
        open={open}
        onOpenChange={setOpen}
        title="Discard changes?"
        description="Your changes will not be saved."
        closeLabel="Cancel"
        returnFocusRef={trigger}
        initialFocusRef={input}
      >
        <Field label="Name">
          <Input
            value={name}
            onValueChange={setName}
            accessibilityLabel="Name"
            focusRef={input}
          />
        </Field>
        <Button
          variant={Variant.Danger}
          onPress={() => {
            setName("");
            setOpen(false);
          }}
        >
          Discard
        </Button>
      </Dialog>
    </UIProvider>
  );
}
root.render(<Gallery />);
