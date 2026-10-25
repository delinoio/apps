// SPDX-License-Identifier: Apache-2.0
#pragma once
#include <functional>
#include <memory>
#include <string>
#include <vector>

namespace lane::desktop {
using Handler = std::function<std::string(const std::string&, const std::vector<std::string>&)>;
}
