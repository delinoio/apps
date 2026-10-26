// SPDX-License-Identifier: Apache-2.0
import { desktopModules, type DesktopBridge } from "../src/desktop";
import { root, useEffect } from "@lynx-js/react";

// Lynxtron filters ordinary BTS console.log output. Test builds forward only
// synthetic smoke markers through an explicit main-process test channel.
function report(message: string) {
  if (!NativeModules.LaneStore && NativeModules.bridge)
    NativeModules.bridge.call("lane.testLog", { message }, () => {});
  else console.log(message);
}
const taskId = "01900000-0000-7000-8000-000000000001";
const requestId = "01900000-0000-7000-8000-000000000002";
function Smoke() {
  useEffect(() => {
    const store =
      NativeModules.LaneStore ||
      desktopModules(NativeModules.bridge as unknown as DesktopBridge).store;
    let spaceId = "";
    let ready = false;
    let closed = false;
    const call = (method: string, request: Record<string, unknown>) =>
      new Promise<any>((resolve, reject) => {
        store.invoke(
          method,
          JSON.stringify({ spaceId, request }),
          (json: string) => {
            const result = JSON.parse(json);
            if (!result.ok) reject(new Error(result.error.reason));
            else resolve(result.response);
          },
        );
      });
    const read = async () => {
      if (!ready || closed) return;
      const response = await call("GetTask", { id: taskId });
      if (response.result.version.task.title === "external commit")
        report("LANE_NATIVE_EXTERNAL_COMMIT_OBSERVED");
    };
    const changed = (event: { spaceId: string }) => {
      report(`LANE_NATIVE_EVENT ${JSON.stringify(event)}`);
      if (event.spaceId === spaceId) void read().catch(fail);
    };
    const fail = (error: Error) =>
      console.error(`LANE_NATIVE_TEST_FAILED ${error.message}`);
    const events = lynx.getJSModule("GlobalEventEmitter");
    events.addListener("lane.storeChanged", changed);
    store.open(
      JSON.stringify({ formatVersion: 2, spaceId: "" }),
      async (json: string) => {
        try {
          const result = JSON.parse(json);
          if (!result.ok) throw new Error(result.error.reason);
          report("LANE_NATIVE_STORE_OPEN");
          spaceId = result.response.spaceId;
          const created = await call("CreateTask", {
            meta: { requestId },
            id: taskId,
            repository: { url: "https://example.com/native/smoke.git" },
            title: "native smoke",
            prompt: "Synthetic Markdown **prompt**",
          });
          if (
            !["RESULT_KIND_APPLIED", "RESULT_KIND_REPLAYED"].includes(
              created.result.result,
            )
          )
            throw new Error("Unexpected creation result");
          const task = await call("GetTask", { id: taskId });
          if (
            task.result.version.task.prompt !== "Synthetic Markdown **prompt**"
          )
            throw new Error("Prompt did not persist");
          const history = await call("ListHistory", { id: taskId });
          if (!history.versions.length) throw new Error("History missing");
          report("LANE_NATIVE_STORE_READY");
          for (const id of ["smoke-input", "smoke-textarea"]) {
            // Use the same imperative value API as the product Field component.
            lynx
              .createSelectorQuery()
              .select(`#${id}`)
              .invoke({
                method: "setValue",
                params: { value: "Native Unicode 한글 😀" },
              })
              .exec();
            await new Promise((resolve) => setTimeout(resolve, 50));
            report(`LANE_NATIVE_INPUT_READ ${id}`);
            const value = await new Promise<any>((resolve, reject) => {
              lynx
                .createSelectorQuery()
                .select(`#${id}`)
                .invoke({
                  method: "getValue",
                  success: resolve,
                  fail: () => reject(new Error(`Native ${id} unavailable`)),
                })
                .exec();
            });
            // Clay 4.1 returns a string; mobile XElement returns {value,...}.
            if (
              (typeof value === "string" ? value : value.value) !==
              "Native Unicode 한글 😀"
            )
              throw new Error(
                `Native ${id} value mismatch: ${JSON.stringify(value)}`,
              );
          }
          report("LANE_NATIVE_INPUT_COMPONENTS_READY");
          ready = true;
          report(
            `LANE_NATIVE_READY ${JSON.stringify({ spaceId, taskId, result: created.result.result })}`,
          );
          void read().catch(fail);
        } catch (error) {
          fail(error as Error);
        }
      },
    );
    return () => {
      closed = true;
      events.removeListener("lane.storeChanged", changed);
      if (spaceId) store.close(spaceId, () => {});
    };
  }, []);
  return (
    <view>
      <input
        id="smoke-input"
        value="Native Unicode 한글 😀"
        style={{ width: "300px", height: "48px" }}
      />
      <textarea
        id="smoke-textarea"
        value="Native Unicode 한글 😀"
        style={{ width: "300px", height: "100px" }}
      />
    </view>
  );
}
root.render(<Smoke />);
