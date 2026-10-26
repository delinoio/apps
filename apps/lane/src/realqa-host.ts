// SPDX-License-Identifier: Apache-2.0
import { desktopModules, type DesktopBridge, type HostModule } from "./desktop";
export enum CaptureMode {
  Screen = "screen",
  Window = "window",
  Region = "region",
  DOM = "dom",
}
export const captureModes = [
  [CaptureMode.Screen, "Full screen"],
  [CaptureMode.Window, "Window"],
  [CaptureMode.Region, "Region"],
  [CaptureMode.DOM, "Chrome element"],
] as const;
export enum EditTool {
  Crop = "crop",
  Draw = "draw",
  Arrow = "arrow",
  Rectangle = "rectangle",
  Text = "text",
  Blur = "blur",
  Cover = "cover",
}
export interface Point {
  x: number;
  y: number;
}
export interface Layer {
  type: Exclude<EditTool, EditTool.Crop>;
  points?: Point[];
  x?: number;
  y?: number;
  width?: number;
  height?: number;
  text?: string;
  color?: string;
  strokeWidth?: number;
  fontSize?: number;
}
export interface Metadata {
  pageUrl?: string;
  pageTitle?: string;
  cssSelector?: string;
}
export interface DraftImage {
  id: string;
  png: string;
  width: number;
  height: number;
  sha256?: string;
  layers: Layer[];
  crop?: { x: number; y: number; width: number; height: number };
  metadata: Metadata;
  canUndo?: boolean;
  canRedo?: boolean;
}
export interface CaptureDraft {
  id?: string;
  requestId?: string;
  images: DraftImage[];
  title: string;
  description: string;
  repositoryId: string;
}
export interface RealQAState {
  window?: "main" | "composer";
  draft: CaptureDraft | null;
  shortcuts: Partial<Record<CaptureMode, string>>;
  permission?: string;
  chrome?: {
    state: string;
    profile?: string;
    extensionId?: string;
    listingUrl?: string;
    message?: string;
    pending?: {
      id: string;
      extensionId: string;
      profileId: string;
      profileName: string;
    }[];
    connections?: {
      profileId: string;
      profileName: string;
      extensionId: string;
      connected: boolean;
    }[];
  };
}
export function desktopCaptureAvailable(): boolean {
  "background only";
  return !NativeModules.LaneOnline && !!NativeModules.bridge;
}
export async function realqa<T = unknown>(
  input: Record<string, unknown>,
): Promise<T> {
  "background only";
  const host = (NativeModules.LaneHost ||
    (NativeModules.bridge &&
      desktopModules(NativeModules.bridge as unknown as DesktopBridge)
        .host)) as HostModule | undefined;
  if (!host?.realqa)
    throw new Error("Capture is available in the Lane desktop app.");
  return new Promise((resolve, reject) =>
    host.realqa!(JSON.stringify(input), (raw) => {
      try {
        const reply = JSON.parse(raw);
        if (!reply.ok)
          throw new Error(
            reply.error?.message || "Capture failed. Retry explicitly.",
          );
        resolve(reply.value as T);
      } catch (error) {
        reject(error);
      }
    }),
  );
}
export function listenRealQA(listeners: Record<string, (event: any) => void>) {
  "background only";
  const events = lynx.getJSModule("GlobalEventEmitter");
  for (const [name, listener] of Object.entries(listeners))
    events.addListener(`lane.realqa.${name}`, listener);
  return () => {
    for (const [name, listener] of Object.entries(listeners))
      events.removeListener(`lane.realqa.${name}`, listener);
  };
}
