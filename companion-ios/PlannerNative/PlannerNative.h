// The functions of the planner-service static library. Keep this file equal to the `#[no_mangle]`
// block of planner/service/src/native.rs: Swift checks each call against it, the linker does not.
#ifndef PLANNER_NATIVE_H
#define PLANNER_NATIVE_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

typedef struct PlannerHandle PlannerHandle;

void planner_response_free(char *response);

PlannerHandle *planner_router_open(const char *root, size_t memory_budget_bytes, char **error);

char *planner_router_call(const PlannerHandle *handle, const char *call, const uint8_t *body, size_t length,
                          uint16_t *status);

void planner_router_cancel(const PlannerHandle *handle);

void planner_router_close(PlannerHandle *handle);

char *planner_routing_plan(const char *request, char **error);

typedef bool (*PlannerProgress)(void *context, uint64_t done, uint64_t total);

bool planner_routing_install(const char *request, PlannerProgress progress, void *context, char **error);

#endif
