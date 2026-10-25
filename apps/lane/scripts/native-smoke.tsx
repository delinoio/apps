// SPDX-License-Identifier: Apache-2.0
import { root, useEffect } from "@lynx-js/react";

const taskId = "01900000-0000-7000-8000-000000000001";
const requestId = "01900000-0000-7000-8000-000000000002";
function Smoke() {
  useEffect(() => {
    let spaceId = "";
    let ready = false;
    let closed = false;
    const call = (method: string, request: Record<string, unknown>) =>
      new Promise<any>((resolve, reject) => {
        NativeModules.LaneStore.invoke(
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
        console.log("LANE_NATIVE_EXTERNAL_COMMIT_OBSERVED");
    };
    const changed = (event: { spaceId: string }) => {
      console.log(`LANE_NATIVE_EVENT ${JSON.stringify(event)}`);
      if (event.spaceId === spaceId) void read().catch(fail);
    };
    const fail = (error: Error) =>
      console.error(`LANE_NATIVE_TEST_FAILED ${error.message}`);
    const events = lynx.getJSModule("GlobalEventEmitter");
    events.addListener("lane.storeChanged", changed);
    NativeModules.LaneStore.open(
      JSON.stringify({ formatVersion: 1, spaceId: "" }),
      async (json: string) => {
        try {
          const result = JSON.parse(json);
          if (!result.ok) throw new Error(result.error.reason);
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
          ready = true;
          console.log(
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
      if (spaceId) NativeModules.LaneStore.close(spaceId, () => {});
    };
  }, []);
  return <view />;
}
root.render(<Smoke />);
