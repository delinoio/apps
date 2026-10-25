// SPDX-License-Identifier: Apache-2.0
// Private implementation helpers; included after the store's JSON validators.
std::string normalizePrefix(const std::string &input) {
  require(!input.empty() && !std::isspace(static_cast<unsigned char>(input.front())) &&
          !std::isspace(static_cast<unsigned char>(input.back())), "Invalid URL prefix.");
  for (size_t pos = 0; pos < input.size();) {
    utf8proc_int32_t cp;
    auto n = utf8proc_iterate(reinterpret_cast<const utf8proc_uint8_t *>(input.data()+pos), input.size()-pos, &cp);
    require(n > 0, "Invalid Unicode URL prefix.");
    auto category = utf8proc_category(cp);
    require(category != UTF8PROC_CATEGORY_CC && cp != '\\', "Invalid URL prefix.");
    if (pos == 0 || pos+size_t(n) == input.size())
      require(category != UTF8PROC_CATEGORY_ZS && category != UTF8PROC_CATEGORY_ZL && category != UTF8PROC_CATEGORY_ZP, "Invalid URL prefix whitespace.");
    pos += size_t(n);
  }
  auto url = ada::parse<ada::url_aggregator>(input);
  require(bool(url), "Invalid URL prefix.");
  require((url->get_protocol() == "http:" || url->get_protocol() == "https:") &&
          !url->get_hostname().empty() && !url->has_credentials(), "Use an HTTP or HTTPS URL without credentials.");
  url->set_search("");
  url->set_hash("");
  std::string path(url->get_pathname());
  while (path.size() > 1 && path.back() == '/') path.pop_back();
  require(url->set_pathname(path));
  return std::string(url->get_href());
}
void captureManifest(const J &c) {
  fields(c,{"attachmentId","width","height","sha256","byteSize","pageUrl","pageTitle","cssSelector"});
  id(str(c,"attachmentId"));
  require(c.contains("width") && c["width"].is_number_integer() && c["width"].get<int64_t>() > 0 && c["width"].get<int64_t>() <= 0x7fffffff &&
          c.contains("height") && c["height"].is_number_integer() && c["height"].get<int64_t>() > 0 && c["height"].get<int64_t>() <= 0x7fffffff, "Invalid capture dimensions.");
  static const std::regex digest("[0-9a-f]{64}"), size("[1-9][0-9]*");
  require(std::regex_match(str(c,"sha256"),digest) && std::regex_match(str(c,"byteSize"),size), "Invalid capture content identity.");
  try { require(std::stoll(str(c,"byteSize")) > 0); } catch (...) { fail("INVALID_INPUT","Invalid capture byte size."); }
  for (auto key : {"pageUrl","pageTitle","cssSelector"}) if (c.contains(key)) str(c,key);
}
std::string decodeBase64(const std::string &input) {
  static const std::string alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
  require(!input.empty() && input.size() % 4 == 0, "Invalid PNG encoding.");
  std::string out;
  for (size_t i = 0; i < input.size(); i += 4) {
    unsigned n = 0; int padding = 0;
    for (size_t j = 0; j < 4; ++j) {
      char c = input[i+j];
      if (c == '=') { require(i+4 == input.size() && j >= 2); ++padding; n <<= 6; }
      else { auto v = alphabet.find(c); require(v != std::string::npos && !padding, "Invalid PNG encoding."); n = (n << 6) | unsigned(v); }
    }
    require(padding <= 2 && (padding != 1 || (n & 255) == 0) && (padding != 2 || (n & 65535) == 0), "Noncanonical PNG encoding.");
    out.push_back(char(n >> 16));
    if (padding < 2) out.push_back(char(n >> 8));
    if (!padding) out.push_back(char(n));
  }
  return out;
}
void validatePNG(const J &c, const std::string &png) {
  captureManifest(c);
  auto u32 = [&](size_t i) { return (uint32_t(uint8_t(png[i])) << 24) | (uint32_t(uint8_t(png[i+1])) << 16) | (uint32_t(uint8_t(png[i+2])) << 8) | uint8_t(png[i+3]); };
  require(png.size() >= 33 && png.compare(0,8,std::string("\x89PNG\r\n\x1a\n",8)) == 0 && u32(8) == 13 && png.substr(12,4) == "IHDR", "A final PNG is required.");
  require(c["width"] == u32(16) && c["height"] == u32(20) && str(c,"sha256") == hash(png) && str(c,"byteSize") == std::to_string(png.size()), "PNG content does not match its reference.");
  const unsigned depth = uint8_t(png[24]), color = uint8_t(png[25]);
  require((color == 0 && (depth == 1 || depth == 2 || depth == 4 || depth == 8 || depth == 16)) ||
          (color == 3 && (depth == 1 || depth == 2 || depth == 4 || depth == 8)) ||
          ((color == 2 || color == 4 || color == 6) && (depth == 8 || depth == 16)), "Invalid PNG pixel format.");
  require(png[26] == 0 && png[27] == 0 && uint8_t(png[28]) <= 1, "Invalid PNG encoding method.");
  bool data = false, endedData = false, palette = false;
  size_t dataBytes = 0;
  for (size_t offset = 8; offset < png.size();) {
    require(png.size()-offset >= 12, "Truncated PNG chunk.");
    size_t size = u32(offset);
    require(size <= png.size()-offset-12, "Truncated PNG chunk.");
    auto kind = png.substr(offset+4,4);
    for (unsigned char ch : kind) require((ch >= 'A' && ch <= 'Z') || (ch >= 'a' && ch <= 'z'), "Invalid PNG chunk type.");
    require((kind[2] & 32) == 0, "Invalid PNG reserved chunk bit.");
    uint32_t crc = ~uint32_t(0);
    for (size_t i = offset+4; i < offset+8+size; ++i) {
      crc ^= uint8_t(png[i]);
      for (int bit = 0; bit < 8; ++bit) crc = (crc >> 1) ^ (0xedb88320u & (0u-(crc & 1)));
    }
    require(~crc == u32(offset+8+size), "Invalid PNG checksum.");
    if (kind == "IHDR") require(offset == 8 && size == 13, "Duplicate PNG header.");
    else if (kind == "PLTE") { require(!palette && !data && size > 0 && size <= 768 && size % 3 == 0 && color != 0 && color != 4, "Invalid PNG palette."); palette = true; }
    else if (kind == "IDAT") { require(!endedData && (color != 3 || palette), "Invalid PNG image data order."); data = true; dataBytes += size; }
    else if (kind == "IEND") { require(size == 0 && data && dataBytes > 0 && offset+12 == png.size(), "Invalid final PNG boundary."); return; }
    else { require((kind[0] & 32) != 0, "Unknown critical PNG chunk."); if (data) endedData = true; }
    offset += size+12;
  }
  fail("INVALID_INPUT","PNG is missing its final boundary.");
}

// Empty repeated fields added in format 2 must not change format 1 receipts.
// Apply to comparison copies only; immutable persisted snapshots stay intact.
J withoutEmptyCaptures(J value) {
  if (value.is_object()) {
    for (auto &entry : value.items()) {
      if (entry.key() == "task" && entry.value().is_object() &&
          entry.value().value("captures",J()) == J::array())
        entry.value().erase("captures");
      if (entry.key() != "payload") entry.value() = withoutEmptyCaptures(entry.value());
    }
  } else if (value.is_array()) {
    for (auto &entry : value) entry = withoutEmptyCaptures(entry);
  }
  return value;
}
