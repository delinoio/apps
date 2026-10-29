// SPDX-License-Identifier: Apache-2.0
// Private implementation helpers for task planning fields, repository planning
// items (labels, milestones, cycles), list filtering and sorting. Included after
// the store's JSON validators; every rule mirrors the shared Go reference
// evaluator and fixtures (protos/lane/v1/planning_*_test.go).
namespace {
constexpr size_t kMaxNameScalars = 100, kMaxDescriptionScalars = 4000,
                 kMaxLabelsPerTask = 100;
constexpr uint64_t kMaxStoryPoints = 2147483647;

// Stable entity kinds shared with the schema CHECK(kind IN (1,2,3,4,5,6)).
enum Kind : int {
  kRepository = 1,
  kTask = 2,
  kUrlConnection = 3,
  kLabel = 4,
  kMilestone = 5,
  kCycle = 6
};
const char *kindName(int kind) {
  switch (kind) {
  case kRepository:
    return "ENTITY_KIND_REPOSITORY";
  case kUrlConnection:
    return "ENTITY_KIND_URL_CONNECTION";
  case kLabel:
    return "ENTITY_KIND_LABEL";
  case kMilestone:
    return "ENTITY_KIND_MILESTONE";
  case kCycle:
    return "ENTITY_KIND_CYCLE";
  default:
    return "ENTITY_KIND_TASK";
  }
}
// Returns 0 for unknown names so callers decide how to reject them.
int kindNumber(const J &kind) {
  if (!kind.is_string())
    return 0;
  for (int k = kRepository; k <= kCycle; ++k)
    if (kind == kindName(k))
      return k;
  return 0;
}
const char *bodyKey(int kind) {
  switch (kind) {
  case kRepository:
    return "repository";
  case kUrlConnection:
    return "urlConnection";
  case kLabel:
    return "label";
  case kMilestone:
    return "milestone";
  case kCycle:
    return "cycle";
  default:
    return "task";
  }
}
bool isPlanningKind(int kind) { return kind >= kLabel && kind <= kCycle; }
int versionKind(const J &v) {
  int k = kindNumber(v.value("kind", J()));
  return k ? k : int(kTask);
}

// A constraint violation is a typed, non-retryable rejection that names the
// violation and, when known, the offending entity.
[[noreturn]] void violation(const std::string &name, const std::string &entity,
                            const std::string &message) {
  J extra = {{"violation", "CONSTRAINT_VIOLATION_" + name}};
  if (!entity.empty())
    extra["entityId"] = entity;
  throw Error("CONSTRAINT_VIOLATION", message, extra);
}

const std::vector<std::string> priorities = {
    "TASK_PRIORITY_URGENT", "TASK_PRIORITY_HIGH", "TASK_PRIORITY_NORMAL",
    "TASK_PRIORITY_LOW"};
const std::vector<std::string> taskTypes = {
    "TASK_TYPE_BUG", "TASK_TYPE_FEATURE", "TASK_TYPE_IMPROVEMENT",
    "TASK_TYPE_DOCUMENTATION", "TASK_TYPE_MAINTENANCE"};
const std::vector<std::string> relationshipKinds = {
    "RELATIONSHIP_KIND_PARENT",     "RELATIONSHIP_KIND_SUBTASKS",
    "RELATIONSHIP_KIND_DEPENDS_ON", "RELATIONSHIP_KIND_BLOCKS",
    "RELATIONSHIP_KIND_RELATED",    "RELATIONSHIP_KIND_DUPLICATE_OF"};
const std::vector<std::string> sortKeys = {
    "TASK_SORT_KEY_PRIORITY",      "TASK_SORT_KEY_TYPE",
    "TASK_SORT_KEY_START_DATE",    "TASK_SORT_KEY_DUE_DATE",
    "TASK_SORT_KEY_STORY_POINTS",  "TASK_SORT_KEY_MILESTONE_NAME",
    "TASK_SORT_KEY_CYCLE_START_DATE"};
const std::vector<std::string> sortDirections = {
    "SORT_DIRECTION_ASCENDING", "SORT_DIRECTION_DESCENDING"};
const std::vector<std::string> constraintViolations = {
    "CONSTRAINT_VIOLATION_PARENT_CYCLE",
    "CONSTRAINT_VIOLATION_DEPENDENCY_CYCLE",
    "CONSTRAINT_VIOLATION_DUPLICATE_CYCLE",
    "CONSTRAINT_VIOLATION_SELF_LINK",
    "CONSTRAINT_VIOLATION_CROSS_REPOSITORY",
    "CONSTRAINT_VIOLATION_REFERENCE_NOT_FOUND",
    "CONSTRAINT_VIOLATION_DUPLICATE_NAME",
    "CONSTRAINT_VIOLATION_LABEL_LIMIT",
    "CONSTRAINT_VIOLATION_DATE_ORDER",
    "CONSTRAINT_VIOLATION_FIELD_RANGE"};

// Optional enum whose UNSPECIFIED value is valid ("unassigned"). Returns 0 for
// an absent or UNSPECIFIED value and the 1-based index otherwise. Names that
// are not declared and numbers outside the defined range are INVALID_INPUT.
int optionalEnum(const J &j, const char *key, const std::string &prefix,
                 const std::vector<std::string> &values) {
  if (!j.contains(key) || j[key].is_null())
    return 0;
  const J &v = j[key];
  if (v.is_number_integer()) {
    auto n = v.get<int64_t>();
    require(n >= 0 && n <= int64_t(values.size()), "Unknown enum value.");
    return int(n);
  }
  require(v.is_string(), "Unknown enum value.");
  auto s = v.get<std::string>();
  if (s == prefix + "UNSPECIFIED")
    return 0;
  auto i = std::find(values.begin(), values.end(), s);
  require(i != values.end(), "Unknown enum value.");
  return int(i - values.begin()) + 1;
}

// Real proleptic Gregorian YYYY-MM-DD date.
bool validDate(const std::string &s) {
  if (s.size() != 10 || s[4] != '-' || s[7] != '-')
    return false;
  for (size_t i = 0; i < s.size(); ++i)
    if (i != 4 && i != 7 && (s[i] < '0' || s[i] > '9'))
      return false;
  int y = std::stoi(s.substr(0, 4)), m = std::stoi(s.substr(5, 2)),
      d = std::stoi(s.substr(8, 2));
  if (m < 1 || m > 12 || d < 1)
    return false;
  static const int days[] = {31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31};
  int limit = days[m - 1];
  if (m == 2 && y % 4 == 0 && (y % 100 != 0 || y % 400 == 0))
    limit = 29;
  return d <= limit;
}
std::string dateField(const J &j, const char *key) {
  require(j[key].is_string() && validDate(j[key].get<std::string>()),
          "Dates must be real YYYY-MM-DD calendar dates.");
  return j[key].get<std::string>();
}

// Decodes UTF-8 into code points; invalid text is INVALID_INPUT.
std::vector<utf8proc_int32_t> codePoints(const std::string &s) {
  std::vector<utf8proc_int32_t> out;
  size_t pos = 0;
  while (pos < s.size()) {
    utf8proc_int32_t cp;
    auto n = utf8proc_iterate(
        reinterpret_cast<const utf8proc_uint8_t *>(s.data() + pos),
        s.size() - pos, &cp);
    require(n > 0, "Invalid Unicode.");
    out.push_back(cp);
    pos += size_t(n);
  }
  return out;
}
std::string encodeCodePoints(const std::vector<utf8proc_int32_t> &points,
                             size_t from, size_t to) {
  std::string out;
  for (size_t i = from; i < to; ++i) {
    utf8proc_uint8_t buffer[4];
    auto n = utf8proc_encode_char(points[i], buffer);
    out.append(reinterpret_cast<char *>(buffer), size_t(n));
  }
  return out;
}
// Unicode White_Space, matching Go strings.TrimSpace and Rust str::trim.
bool whiteSpace(utf8proc_int32_t cp) {
  return (cp >= 9 && cp <= 13) || cp == 32 || cp == 0x85 || cp == 0xA0 ||
         cp == 0x1680 || (cp >= 0x2000 && cp <= 0x200A) || cp == 0x2028 ||
         cp == 0x2029 || cp == 0x202F || cp == 0x205F || cp == 0x3000;
}
std::string trimmed(const std::string &s) {
  auto points = codePoints(s);
  size_t from = 0, to = points.size();
  while (from < to && whiteSpace(points[from]))
    ++from;
  while (to > from && whiteSpace(points[to - 1]))
    --to;
  return encodeCodePoints(points, from, to);
}
// Unicode simple lowercase (per code point), used for name uniqueness and
// milestone-name sorting. Not the full case fold of search.
std::string lowerSimple(const std::string &s) {
  auto points = codePoints(s);
  for (auto &cp : points)
    cp = utf8proc_tolower(cp);
  return encodeCodePoints(points, 0, points.size());
}

std::vector<std::string> sortedUnique(std::vector<std::string> in) {
  std::sort(in.begin(), in.end());
  in.erase(std::unique(in.begin(), in.end()), in.end());
  return in;
}
// Reads an optional array of UUID v7 strings as a sorted unique set.
std::vector<std::string> idSet(const J &j, const char *key) {
  std::vector<std::string> out;
  if (!j.contains(key) || j[key].is_null())
    return out;
  require(j[key].is_array(), "An ID list is required.");
  for (const auto &v : j[key]) {
    require(v.is_string());
    id(v.get<std::string>());
    out.push_back(v.get<std::string>());
  }
  return sortedUnique(std::move(out));
}
std::string optionalId(const J &j, const char *key) {
  if (!j.contains(key) || j[key].is_null())
    return "";
  require(j[key].is_string());
  auto value = j[key].get<std::string>();
  id(value);
  return value;
}
// Story points are an unsigned 31-bit value on the wire; absent differs from 0.
uint64_t storyPoints(const J &v) {
  if (v.is_number_unsigned())
    return v.get<uint64_t>();
  if (v.is_number_integer() || v.is_number_float())
    violation("FIELD_RANGE", "", "Story points must be a whole number from 0.");
  if (v.is_string()) {
    auto s = v.get<std::string>();
    if (s.empty() || s.size() > 20 ||
        s.find_first_not_of("0123456789") != std::string::npos)
      violation("FIELD_RANGE", "", "Story points must be a whole number from 0.");
    try {
      return std::stoull(s);
    } catch (...) {
      violation("FIELD_RANGE", "", "Story points are out of range.");
    }
  }
  fail("INVALID_INPUT", "Story points must be a number.");
}

// Task-local planning rules, independent of other entities. Copies the
// normalized planning fields of `t` into `r`; cross-entity rules (references,
// graph) are checked by the store inside the write transaction.
void planningFieldsShape(const J &t, J &r) {
  if (int p = optionalEnum(t, "priority", "TASK_PRIORITY_", priorities))
    r["priority"] = priorities[p - 1];
  if (int p = optionalEnum(t, "type", "TASK_TYPE_", taskTypes))
    r["type"] = taskTypes[p - 1];
  for (auto key : {"startDate", "dueDate"})
    if (t.contains(key) && !t[key].is_null())
      r[key] = dateField(t, key);
  if (t.contains("storyPoints") && !t["storyPoints"].is_null()) {
    auto points = storyPoints(t["storyPoints"]);
    if (points > kMaxStoryPoints)
      violation("FIELD_RANGE", "", "Story points exceed 2147483647.");
    r["storyPoints"] = points;
  }
  if (r.contains("startDate") && r.contains("dueDate") &&
      r["startDate"].get<std::string>() > r["dueDate"].get<std::string>())
    violation("DATE_ORDER", "", "The start date is after the due date.");
  auto labels = idSet(t, "labelIds");
  if (labels.size() > kMaxLabelsPerTask)
    violation("LABEL_LIMIT", "", "A task can have at most 100 labels.");
  if (!labels.empty())
    r["labelIds"] = labels;
  for (auto key : {"milestoneId", "cycleId", "parentTaskId", "duplicateOfTaskId"}) {
    auto value = optionalId(t, key);
    if (!value.empty())
      r[key] = value;
  }
  for (auto key : {"dependsOnTaskIds", "relatedTaskIds"}) {
    auto set = idSet(t, key);
    if (!set.empty())
      r[key] = set;
  }
}
const std::initializer_list<const char *> planningTaskFields = {
    "priority",     "type",         "labelIds",          "startDate",
    "dueDate",      "storyPoints",  "milestoneId",       "cycleId",
    "parentTaskId", "dependsOnTaskIds", "relatedTaskIds", "duplicateOfTaskId"};
bool hasPlanningFields(const J &task) {
  for (auto key : planningTaskFields)
    if (task.contains(key))
      return true;
  return false;
}

// Normalizes a RelationshipEdits object: sorted unique IDs, empty lists and an
// empty object omitted so equal edits have equal fingerprints.
J normalizedEdits(const J &edits) {
  fields(edits, {"addSubtaskIds", "removeSubtaskIds", "addBlockedTaskIds",
                 "removeBlockedTaskIds"});
  J out = J::object();
  for (auto key : {"addSubtaskIds", "removeSubtaskIds", "addBlockedTaskIds",
                   "removeBlockedTaskIds"}) {
    auto set = idSet(edits, key);
    if (!set.empty())
      out[key] = set;
  }
  return out;
}

// Normalizes a label, milestone or cycle body: trimmed names, canonical color,
// valid dates. Empty strings and false booleans are omitted like ProtoJSON.
J planningShape(int kind, const J &b) {
  require(b.is_object());
  switch (kind) {
  case kLabel:
    fields(b, {"repositoryId", "name", "color", "description"});
    break;
  case kMilestone:
    fields(b, {"repositoryId", "name", "description", "targetDate", "completed"});
    break;
  default:
    fields(b, {"repositoryId", "name", "description", "startDate", "endDate"});
  }
  auto repository = str(b, "repositoryId");
  id(repository);
  auto name = trimmed(str(b, "name"));
  auto nameLength = codePoints(name).size();
  require(nameLength >= 1 && nameLength <= kMaxNameScalars,
          "A name of 1 to 100 characters is required.");
  auto description = str(b, "description");
  require(codePoints(description).size() <= kMaxDescriptionScalars,
          "A description can have at most 4000 characters.");
  J r = {{"repositoryId", repository}, {"name", name}};
  if (!description.empty())
    r["description"] = description;
  if (kind == kLabel) {
    static const std::regex color("#[0-9A-Fa-f]{6}");
    auto value = str(b, "color");
    require(std::regex_match(value, color), "A #RRGGBB color is required.");
    for (auto &c : value)
      c = char(std::toupper(static_cast<unsigned char>(c)));
    r["color"] = value;
  } else if (kind == kMilestone) {
    if (b.contains("targetDate") && !b["targetDate"].is_null())
      r["targetDate"] = dateField(b, "targetDate");
    if (b.contains("completed") && !b["completed"].is_null()) {
      require(b["completed"].is_boolean());
      if (b["completed"].get<bool>())
        r["completed"] = true;
    }
  } else {
    require(b.contains("startDate") && b.contains("endDate"),
            "A cycle needs start and end dates.");
    auto start = dateField(b, "startDate"), end = dateField(b, "endDate");
    if (start > end)
      violation("DATE_ORDER", "", "The cycle starts after it ends.");
    r["startDate"] = start;
    r["endDate"] = end;
  }
  return r;
}

// One list sort key: unassigned values sort last in both directions.
struct SortValue {
  bool assigned = false;
  int64_t number = 0;
  std::string text;
};
int compareTuples(const SortValue &a, const SortValue &b, const std::string &aId,
                  const std::string &bId, bool descending) {
  if (a.assigned != b.assigned)
    return a.assigned ? -1 : 1;
  if (a.assigned) {
    int c = 0;
    if (a.number != b.number)
      c = a.number < b.number ? -1 : 1;
    else if (a.text != b.text)
      c = a.text < b.text ? -1 : 1; // std::string compares bytes: UTF-8 code point order.
    if (descending)
      c = -c;
    if (c != 0)
      return c;
  }
  return aId < bId ? -1 : aId > bId ? 1 : 0;
}
std::string hexEncode(const std::string &s) {
  static const char *digits = "0123456789abcdef";
  std::string out;
  for (unsigned char c : s) {
    out += digits[c >> 4];
    out += digits[c & 15];
  }
  return out;
}
std::string hexDecode(const std::string &s) {
  require(s.size() % 2 == 0, "Invalid page cursor.");
  std::string out;
  auto nibble = [](char c) -> int {
    if (c >= '0' && c <= '9')
      return c - '0';
    if (c >= 'a' && c <= 'f')
      return c - 'a' + 10;
    return -1;
  };
  for (size_t i = 0; i < s.size(); i += 2) {
    int hi = nibble(s[i]), lo = nibble(s[i + 1]);
    require(hi >= 0 && lo >= 0, "Invalid page cursor.");
    out += char((hi << 4) | lo);
  }
  return out;
}
} // namespace
