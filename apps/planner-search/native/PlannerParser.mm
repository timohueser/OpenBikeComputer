#import <Foundation/Foundation.h>
#include <Python/Python.h>
#include <algorithm>
#include <chrono>
#include <mutex>
#include <onnxruntime_cxx_api.h>
#include <stdexcept>
#include <string>
#include <vector>

extern "C" {
void *obc_tokenizer_create(const char *, char **);
char *obc_tokenizer_encode(void *, const char *, size_t, char **);
void obc_tokenizer_destroy(void *);
void obc_tokenizer_string_free(char *);
}
using Clock = std::chrono::steady_clock;
static double milliseconds(Clock::time_point start) {
  return std::chrono::duration<double, std::milli>(Clock::now() - start)
      .count();
}
static std::runtime_error pythonError() {
  PyObject *error = PyErr_GetRaisedException();
  PyObject *string = error ? PyObject_Str(error) : nullptr;
  std::string text =
      string ? PyUnicode_AsUTF8(string) : "Python operation failed";
  Py_XDECREF(string);
  Py_XDECREF(error);
  return std::runtime_error(text);
}
static void status(PyStatus value) {
  if (PyStatus_Exception(value))
    throw std::runtime_error(value.err_msg ? value.err_msg
                                           : "Python initialization failed");
}
static void initializePython(NSString *bundle) {
  static std::mutex initialization;
  std::lock_guard<std::mutex> lock(initialization);
  if (Py_IsInitialized())
    return;
  PyPreConfig pre;
  PyPreConfig_InitIsolatedConfig(&pre);
  pre.utf8_mode = 1;
  status(Py_PreInitialize(&pre));
  PyConfig config;
  PyConfig_InitIsolatedConfig(&config);
  config.write_bytecode = 0;
  config.buffered_stdio = 0;
  config.module_search_paths_set = 1;
  status(PyConfig_SetBytesString(
      &config, &config.home,
      [[bundle stringByAppendingPathComponent:@"python"] UTF8String]));
  for (NSString *part in @[
         @"python/lib/python3.14", @"python/lib/python3.14/lib-dynload", @"app"
       ]) {
    wchar_t *path = Py_DecodeLocale(
        [[bundle stringByAppendingPathComponent:part] UTF8String], nullptr);
    status(PyWideStringList_Append(&config.module_search_paths, path));
    PyMem_RawFree(path);
  }
  PyStatus initialized = Py_InitializeFromConfig(&config);
  PyConfig_Clear(&config);
  status(initialized);
  if (PyRun_SimpleString(
          "import os\nos.environ['RAPIDFUZZ_IMPLEMENTATION']='python'\n"))
    throw pythonError();
  PyEval_SaveThread();
}
struct PythonLock {
  PyGILState_STATE state;
  double initializationMs;
  PythonLock(NSString *bundle) {
    auto start = Clock::now();
    initializePython(bundle);
    state = PyGILState_Ensure();
    initializationMs = milliseconds(start);
  }
  ~PythonLock() { PyGILState_Release(state); }
};
struct NativeParser {
  std::mutex mutex;
  Ort::Env env{ORT_LOGGING_LEVEL_WARNING, "planner-parser"};
  Ort::Session session{nullptr};
  void *tokenizer = nullptr;
  PyObject *decode = nullptr, *validate = nullptr, *dumps = nullptr;
  double pythonMs = 0, tokenizerMs = 0, modelMs = 0;
  size_t intentCount = 0, tagCount = 0;
  NativeParser(NSString *root, NSString *bundle) {
    PythonLock python(bundle);
    try {
      env.DisableTelemetryEvents();
      auto start = Clock::now();
      PyObject *pathlib = PyImport_ImportModule("pathlib");
      PyObject *artifacts = PyImport_ImportModule("artifacts");
      if (!pathlib || !artifacts) {
        Py_XDECREF(pathlib);
        Py_XDECREF(artifacts);
        throw pythonError();
      }
      PyObject *path =
          PyObject_CallMethod(pathlib, "Path", "s", root.UTF8String);
      Py_DECREF(pathlib);
      PyObject *checked =
          path ? PyObject_CallMethod(artifacts, "check_labels", "O", path)
               : nullptr;
      Py_XDECREF(path);
      Py_DECREF(artifacts);
      if (!checked)
        throw pythonError();
      Py_DECREF(checked);
      PyObject *schema = PyImport_ImportModule("schema");
      if (!schema)
        throw pythonError();
      PyObject *intents = PyObject_GetAttrString(schema, "INTENTS");
      PyObject *labels = PyObject_GetAttrString(schema, "LABELS");
      Py_DECREF(schema);
      intentCount = intents ? PyObject_Length(intents) : 0;
      tagCount = labels ? PyObject_Length(labels) : 0;
      Py_XDECREF(intents);
      Py_XDECREF(labels);
      if (!intentCount || !tagCount)
        throw pythonError();
      PyObject *module = PyImport_ImportModule("prediction");
      if (!module)
        throw pythonError();
      decode = PyObject_GetAttrString(module, "request_from_prediction");
      validate = PyObject_GetAttrString(module, "validate_text");
      Py_DECREF(module);
      module = PyImport_ImportModule("json");
      if (!module)
        throw pythonError();
      dumps = PyObject_GetAttrString(module, "dumps");
      Py_DECREF(module);
      if (!decode || !validate || !dumps)
        throw pythonError();
      pythonMs = python.initializationMs + milliseconds(start);
      start = Clock::now();
      char *error = nullptr;
      tokenizer = obc_tokenizer_create(
          [[root stringByAppendingPathComponent:@"tokenizer.json"] UTF8String],
          &error);
      if (!tokenizer) {
        std::string text = error ? error : "Tokenizer initialization failed";
        obc_tokenizer_string_free(error);
        throw std::runtime_error(text);
      }
      tokenizerMs = milliseconds(start);
      start = Clock::now();
      Ort::SessionOptions options;
      options.SetIntraOpNumThreads(1);
      options.SetGraphOptimizationLevel(GraphOptimizationLevel::ORT_ENABLE_ALL);
      session = Ort::Session(
          env,
          [[root stringByAppendingPathComponent:@"model.int8.onnx"] UTF8String],
          options);
      modelMs = milliseconds(start);
    } catch (...) {
      release();
      throw;
    }
  }
  ~NativeParser() { release(); }
  void release() {
    const auto gil = PyGILState_Ensure();
    if (tokenizer)
      obc_tokenizer_destroy(tokenizer);
    Py_XDECREF(decode);
    Py_XDECREF(validate);
    Py_XDECREF(dumps);
    PyGILState_Release(gil);
  }
  NSDictionary *parse(NSString *text) {
    std::lock_guard<std::mutex> lock(mutex);
    struct GIL {
      PyGILState_STATE state = PyGILState_Ensure();
      ~GIL() { PyGILState_Release(state); }
    } gil;
    struct Text {
      PyObject *value;
      ~Text() { Py_XDECREF(value); }
    } pythonText{PyUnicode_FromStringAndSize(
        [text UTF8String],
        [text lengthOfBytesUsingEncoding:NSUTF8StringEncoding])};
    if (!pythonText.value)
      throw pythonError();
    PyObject *valid =
        PyObject_CallFunctionObjArgs(validate, pythonText.value, nullptr);
    if (!valid)
      throw pythonError();
    Py_DECREF(valid);
    char *error = nullptr;
    char *encoded = obc_tokenizer_encode(
        tokenizer, [text UTF8String],
        [text lengthOfBytesUsingEncoding:NSUTF8StringEncoding], &error);
    if (!encoded) {
      std::string message = error ? error : "Tokenization failed";
      obc_tokenizer_string_free(error);
      throw std::runtime_error(message);
    }
    NSData *data = [NSData dataWithBytes:encoded length:strlen(encoded)];
    obc_tokenizer_string_free(encoded);
    NSDictionary *encoding = [NSJSONSerialization JSONObjectWithData:data
                                                             options:0
                                                               error:nil];
    NSArray *idsArray = encoding[@"ids"], *offsetsArray = encoding[@"offsets"];
    std::vector<int64_t> ids;
    for (NSNumber *idValue in idsArray)
      ids.push_back(idValue.longLongValue);
    if (ids.empty() || ids.size() > 64)
      throw std::runtime_error("Invalid token count");
    std::vector<int64_t> mask(ids.size(), 1);
    int64_t shape[] = {1, (int64_t)ids.size()};
    auto memory =
        Ort::MemoryInfo::CreateCpu(OrtArenaAllocator, OrtMemTypeDefault);
    std::vector<Ort::Value> inputs;
    inputs.push_back(Ort::Value::CreateTensor<int64_t>(memory, ids.data(),
                                                       ids.size(), shape, 2));
    inputs.push_back(Ort::Value::CreateTensor<int64_t>(memory, mask.data(),
                                                       mask.size(), shape, 2));
    const char *inputNames[] = {"input_ids", "attention_mask"};
    const char *outputNames[] = {"intent_logits", "tag_logits"};
    auto result = session.Run(Ort::RunOptions{nullptr}, inputNames,
                              inputs.data(), 2, outputNames, 2);
    if (result[0].GetTensorTypeAndShapeInfo().GetShape() !=
            std::vector<int64_t>{1, (int64_t)intentCount} ||
        result[1].GetTensorTypeAndShapeInfo().GetShape() !=
            std::vector<int64_t>{1, (int64_t)ids.size(), (int64_t)tagCount})
      throw std::runtime_error(
          "Parser model output differs from its label schema");
    const float *intentLogits = result[0].GetTensorData<float>();
    const float *tagLogits = result[1].GetTensorData<float>();
    PyObject *offsets = PyList_New(offsetsArray.count);
    PyObject *tags = PyList_New(ids.size());
    for (NSUInteger i = 0; i < offsetsArray.count; i++) {
      NSArray *pair = offsetsArray[i];
      PyList_SET_ITEM(
          offsets, i,
          Py_BuildValue("(ii)", [pair[0] intValue], [pair[1] intValue]));
    }
    for (size_t i = 0; i < ids.size(); i++)
      PyList_SET_ITEM(
          tags, i,
          PyLong_FromLong(std::max_element(tagLogits + i * tagCount,
                                           tagLogits + (i + 1) * tagCount) -
                          (tagLogits + i * tagCount)));
    PyObject *intent = PyLong_FromLong(
        std::max_element(intentLogits, intentLogits + intentCount) -
        intentLogits);
    PyObject *request = PyObject_CallFunctionObjArgs(
        decode, pythonText.value, offsets, intent, tags, nullptr);
    Py_DECREF(offsets);
    Py_DECREF(intent);
    Py_DECREF(tags);
    if (!request)
      throw pythonError();
    PyObject *json = PyObject_CallFunctionObjArgs(dumps, request, nullptr);
    Py_DECREF(request);
    if (!json)
      throw pythonError();
    const char *utf8 = PyUnicode_AsUTF8(json);
    NSData *bytes = [NSData dataWithBytes:utf8 length:strlen(utf8)];
    Py_DECREF(json);
    return [NSJSONSerialization JSONObjectWithData:bytes options:0 error:nil];
  }
};
extern "C" void *planner_parser_create(const char *directory,
                                       const char *bundleDirectory,
                                       char **error) {
  @autoreleasepool {
    try {
      return new NativeParser([NSString stringWithUTF8String:directory],
                              [NSString stringWithUTF8String:bundleDirectory]);
    } catch (const std::exception &failure) {
      *error = strdup(failure.what());
      return nullptr;
    }
  }
}
extern "C" char *planner_parser_parse(void *handle, const char *text,
                                      size_t length) {
  @autoreleasepool {
    NSDictionary *result;
    try {
      auto start = Clock::now();
      NSDictionary *request = static_cast<NativeParser *>(handle)->parse(
          [[NSString alloc] initWithBytes:text
                                   length:length
                                 encoding:NSUTF8StringEncoding]);
      result = @{@"request" : request, @"elapsed" : @(milliseconds(start))};
    } catch (const std::exception &error) {
      result = @{@"error" : @(error.what())};
    }
    NSData *data = [NSJSONSerialization dataWithJSONObject:result
                                                   options:0
                                                     error:nil];
    return strdup([[NSString alloc] initWithData:data
                                        encoding:NSUTF8StringEncoding]
                      .UTF8String);
  }
}
extern "C" void planner_parser_destroy(void *handle) {
  delete static_cast<NativeParser *>(handle);
}
extern "C" char *planner_parser_info(void *handle) {
  @autoreleasepool {
    auto *parser = static_cast<NativeParser *>(handle);
    NSDictionary *info = @{
      @"python_initialization_ms" : @(parser->pythonMs),
      @"tokenizer_initialization_ms" : @(parser->tokenizerMs),
      @"model_initialization_ms" : @(parser->modelMs),
      @"onnx_runtime" : @(OrtGetApiBase()->GetVersionString()),
      @"python_runtime" : @(Py_GetVersion())
    };
    NSData *data = [NSJSONSerialization dataWithJSONObject:info
                                                   options:0
                                                     error:nil];
    return strdup([[NSString alloc] initWithData:data
                                        encoding:NSUTF8StringEncoding]
                      .UTF8String);
  }
}

extern "C" char *planner_python_run(const char *moduleName,
                                    const char *directory,
                                    const char *bundleDirectory) {
  @autoreleasepool {
    try {
      PythonLock python([NSString stringWithUTF8String:bundleDirectory]);
      PyObject *module = PyImport_ImportModule(moduleName);
      if (!module)
        throw pythonError();
      PyObject *result = PyObject_CallMethod(module, "run", "s", directory);
      Py_DECREF(module);
      if (!result)
        throw pythonError();
      PyObject *json = PyImport_ImportModule("json");
      if (!json) {
        Py_DECREF(result);
        throw pythonError();
      }
      PyObject *text = PyObject_CallMethod(json, "dumps", "O", result);
      Py_DECREF(json);
      Py_DECREF(result);
      if (!text)
        throw pythonError();
      char *output = strdup(PyUnicode_AsUTF8(text));
      Py_DECREF(text);
      return output;
    } catch (const std::exception &error) {
      NSData *data = [NSJSONSerialization dataWithJSONObject:@{
        @"error" : @(error.what())
      }
                                                     options:0
                                                       error:nil];
      return strdup([[NSString alloc] initWithData:data
                                          encoding:NSUTF8StringEncoding]
                        .UTF8String);
    }
  }
}
