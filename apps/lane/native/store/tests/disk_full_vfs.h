// SPDX-License-Identifier: Apache-2.0
// Inject SQLITE_FULL at the real SQLite file-write boundary. Other VFS and
// file methods remain the bundled SQLite implementation, including WAL locks.
struct DiskFullVfs {
  struct File {
    const sqlite3_io_methods* original;
    sqlite3_io_methods patched;
    std::string path;
  };
  inline static DiskFullVfs* current = nullptr;
  sqlite3_vfs* original = sqlite3_vfs_find(nullptr);
  sqlite3_vfs vfs = *original;
  std::map<sqlite3_file*, std::unique_ptr<File>> files;
  std::string failing_root;
  bool armed = false;
  unsigned failures = 0;
  DiskFullVfs() {
    require(current == nullptr, "one fault VFS at a time");
    current = this;
    vfs.zName = "lane-test-disk-full";
    vfs.xOpen = open;
    require(sqlite3_vfs_register(&vfs, 1) == SQLITE_OK, "register fault VFS");
  }
  ~DiskFullVfs() {
    sqlite3_vfs_register(original, 1);
    sqlite3_vfs_unregister(&vfs);
    current = nullptr;
  }
  static int open(sqlite3_vfs*, const char* path, sqlite3_file* file, int flags, int* output) {
    auto& self = *current;
    const int rc = self.original->xOpen(self.original, path, file, flags, output);
    if (rc != SQLITE_OK) return rc;
    auto entry = std::make_unique<File>();
    entry->original = file->pMethods;
    entry->patched = *file->pMethods;
    entry->path = path ? path : "";
    entry->patched.xWrite = write;
    entry->patched.xClose = close;
    file->pMethods = &entry->patched;
    self.files.emplace(file, std::move(entry));
    return rc;
  }
  static int write(sqlite3_file* file, const void* data, int size, sqlite3_int64 offset) {
    auto& self = *current;
    const auto& entry = *self.files.at(file);
    if (self.armed && entry.path.rfind(self.failing_root, 0) == 0) {
      ++self.failures;
      return SQLITE_FULL;
    }
    return entry.original->xWrite(file, data, size, offset);
  }
  static int close(sqlite3_file* file) {
    auto& self = *current;
    const auto* original_methods = self.files.at(file)->original;
    file->pMethods = original_methods;
    const int rc = original_methods->xClose(file);
    self.files.erase(file);
    return rc;
  }
};
