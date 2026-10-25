// SPDX-License-Identifier: Apache-2.0
#include "../lane_store.h"
#include "../vendor/json/json.hpp"
#include <iostream>
int main(int argc, char **argv) {
  if (argc != 2)
    return 2;
  lane::Store store(argv[1]);
  std::string line;
  while (std::getline(std::cin, line)) {
    try {
      auto q = nlohmann::json::parse(line);
      auto m = q.value("method", "");
      std::string out;
      if (m == "open")
        out = store.open(q.value("request", nlohmann::json{{"formatVersion", 2},
                                                           {"spaceId", ""}})
                             .dump());
      else if (m == "migrate")
        out = store.migrate(q.at("spaceId"));
      else if (m == "approveAccount")
        out = store.approveAccount(q.at("request").dump(), q.value("merge",false));
      else if (m == "acquireSync")
        out = store.acquireSync(q.at("spaceId"),q.at("owner"));
      else if (m == "applySync")
        out = store.applySync(q.at("spaceId"),q.at("request").dump());
      else if (m == "pendingSync")
        out = store.pendingSync(q.at("spaceId"),q.at("owner"));
      else if (m == "acknowledgeAttachment")
        out = store.acknowledgeAttachment(q.at("spaceId"),q.at("owner"),q.at("epoch"),q.at("taskId"),q.at("attachmentId"));
      else if (m == "cacheAttachment")
        out = store.cacheAttachment(q.at("spaceId"),q.at("taskId"),q.at("attachmentId"),q.at("png"));
      else if (m == "close")
        out = store.close(q.at("spaceId"));
      else if (m == "poll")
        out = store.poll();
      else if (m == "invoke")
        out = store.invoke(
            q.at("operation"),
            nlohmann::json{
                {"spaceId", q.at("spaceId")},
                {"request", q.value("request", nlohmann::json::object())}}
                .dump());
      else if (m == "exit") {
        std::cout << R"({"ok":true,"response":{"completed":true}})"
                  << std::endl;
        return 0;
      } else
        return 2;
      std::cout << out << std::endl;
    } catch (...) {
      std::cout
          << R"({"ok":false,"error":{"reason":"ERROR_REASON_INVALID_INPUT"}})"
          << std::endl;
    }
  }
}
