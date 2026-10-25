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
        out = store.open(q.value("request", nlohmann::json{{"formatVersion", 1},
                                                           {"spaceId", ""}})
                             .dump());
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
      else
        return 2;
      std::cout << out << std::endl;
    } catch (...) {
      std::cout
          << R"({"ok":false,"error":{"reason":"ERROR_REASON_INVALID_INPUT"}})"
          << std::endl;
    }
  }
}
