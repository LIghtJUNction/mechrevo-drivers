// SPDX-License-Identifier: GPL-2.0-or-later
// Execute pinned upstream thermald functions against in-memory services.
// No sysfs path is opened by this executable. PID/critical actions abort.
#include <algorithm>
#include <cerrno>
#include <climits>
#include <cstring>
#include <iostream>
#include <map>
#include <memory>
#include <mutex>
#include <sstream>
#include <string>
#include <vector>
#include "thd_cdev_rapl.h"
#include "thd_trip_point.h"

struct fake_meter {
    int power = 40000000;
    void rapl_start_measure_power() {}
    int rapl_action_get_power(int) { return power; }
};
struct fake_sensor {
    void sensor_poll_trip(bool) { std::abort(); }
    void sensor_fast_poll(bool) { std::abort(); }
    bool check_async_capable() { std::abort(); }
    void set_threshold(int, unsigned) { std::abort(); }
};
using cthd_sensor = fake_sensor;
struct fake_engine {
    fake_meter rapl_power_meter;
    cthd_sensor *get_sensor(int) { std::abort(); }
};
static fake_engine engine;
static fake_engine *thd_engine = &engine;
static constexpr int PACKAGE = 0;
static bool ignore_critical = false;
static constexpr int RB_POWER_OFF = 0;
static void sync() { std::abort(); }
static int reboot(int) { std::abort(); }

// Only the PPCC discovery service is doubled. Tests explicitly model the
// no-PPCC platform; production must refuse this route if PPCC appears.
bool cthd_sysfs_cdev_rapl::read_ppcc_power_limits() { return false; }

/* @ACTUAL_CDEV@ */
/* @ACTUAL_RAPL@ */
/* @ACTUAL_TRIP@ */

// Snapshot registration is outside these controller tests. The separately
// audited upstream atexit restoration can temporarily restore initial PL1.
void cthd_sysfs_cdev_rapl::register_for_restoration() {}

static void require(bool ok, const char *why) {
    if (!ok) { std::cerr << why << '\n'; std::exit(1); }
}

class test_rapl : public cthd_sysfs_cdev_rapl {
public:
    test_rapl() : cthd_sysfs_cdev_rapl(1, 0) {
        set_cdev_type("rapl_controller");
        set_debounce_interval(-1);
    }
    fake_state &hardware() { return cdev_sysfs.state(); }
    int native_step() const { return inc_dec_val; }
    int saved_power() const { return power_on_constraint_0_pwr; }
    bool is_bios_locked() const { return bios_locked; }
    void pair(int ceiling = 55000000, int floor = 27500000) {
        thd_cdev_set_min_state_param(ceiling);
        thd_cdev_set_max_state_param(floor);
    }
};

class next_cdev : public cthd_cdev {
public:
    int writes = 0;
    next_cdev() : cthd_cdev(2, "next") {
        set_cdev_type("next_thermal_device");
        set_min_state(0); set_max_state(100); set_inc_dec_value(1);
        set_debounce_interval(-1);
    }
    void set_curr_state(int state, int) override { curr_state = state; ++writes; }
};

static void initialise(test_rapl &r) {
    require(r.update() == THD_SUCCESS, "actual update failed");
    require(r.get_min_state() == 140000000, "native min must follow initial PL1");
    require(r.get_max_state() == 70000000, "actual native floor must be 50 percent");
    require(r.native_step() == -7000000, "native step changed");
    require(r.saved_power() == 0, "valid max must not capture fallback power");
}

int main(int argc, char **argv) {
    require(argc == 2, "case required");
    const std::string name = argv[1];
    fake_files.clear();
    test_rapl r;
    if (name == "invalid_max_restore") {
        r.hardware().numbers["constraint_0_max_power_uw"] = -1;
        require(r.update() == THD_SUCCESS, "fallback should be observable");
        require(r.saved_power() == 140000000, "fallback must capture initial power");
        r.pair();
        r.set_curr_state(40000000, 1);
        r.set_curr_state(55000000, 0);
        require(r.hardware().numbers["constraint_0_power_limit_uw"] == 140000000,
                "counterexample: invalid max bypasses XML ceiling");
        require(r.hardware().numbers["enabled"] == 0,
                "counterexample: fallback also disables RAPL");
        return 0;
    }
    if (name == "initially_disabled") r.hardware().numbers["enabled"] = 0;
    initialise(r);
    if (name == "native_range") return 0;
    if (name == "sub_tdp_ceiling") {
        for (int ceiling : {35000000, 45000000}) {
            r.pair(ceiling, ceiling / 2);
            r.set_curr_state(ceiling - 7000000, 1);
            require(r.hardware().numbers["constraint_0_power_limit_uw"] == ceiling - 7000000,
                    "lower cap blocked further thermal reduction");
            r.set_curr_state(1, 1);
            require(r.get_curr_state() == ceiling / 2 && r.in_max_state(),
                    "lower cap did not reach its paired thermal floor");
            next_cdev next;
            cthd_trip_point trip(0, PASSIVE, 90000, 1000, 0, DEFAULT_SENSOR_ID, SEQUENTIAL);
            trip.thd_trip_point_add_cdev(r, 10);
            trip.thd_trip_point_add_cdev(next, 0);
            bool reset = false;
            require(trip.thd_trip_point_check(DEFAULT_SENSOR_ID, 95000,
                    PREF_ENERGY_CONSERVE, &reset), "lower cap trip failed");
            require(next.writes == 1, "lower cap blocked the next cooling device");
            r.set_curr_state(ceiling, 0);
            require(r.hardware().numbers["constraint_0_power_limit_uw"] == ceiling,
                    "lower cap release exceeded its configured ceiling");
        }
        return 0;
    }
    if (name == "original_policy") {
        r.set_curr_state(100000000, 1);
        r.set_curr_state(140000000, 0);
        require(r.hardware().numbers["constraint_0_power_limit_uw"] == 140000000,
                "unmodified native release did not restore its own max");
        return 0;
    }
    if (name == "min_only_counterexample") {
        r.thd_cdev_set_min_state_param(55000000);
        r.set_curr_state(40000000, 1);
        require(r.get_max_state() == 70000000, "MinState-only unexpectedly changes floor");
        require(r.hardware().numbers["constraint_0_power_limit_uw"] == 55000000,
                "counterexample: MinState-only must block requested 40 W");
        return 0;
    }
    if (name == "sequential_min_only_counterexample")
        r.thd_cdev_set_min_state_param(55000000);
    else
        r.pair();
    require(r.get_curr_state() == 55000000, "actual min setter must update cache");
    require(r.native_step() == -7000000, "pair must retain existing step");
    if (name == "thermal_lower_release") {
        r.set_curr_state(40000000, 1);
        require(r.hardware().numbers["constraint_0_power_limit_uw"] == 40000000,
                "thermal request must remain able to lower below user ceiling");
        r.set_curr_state(55000000, 0);
        require(r.hardware().numbers["constraint_0_power_limit_uw"] == 55000000,
                "release must respect ceiling");
        require(r.hardware().numbers["enabled"] == 1, "release disabled active RAPL");
    } else if (name == "range_boundaries") {
        r.set_curr_state(1, 1);
        require(r.hardware().numbers["constraint_0_power_limit_uw"] == 27500000,
                "actual lower range clamp failed");
        r.set_curr_state(999000000, 0);
        require(r.hardware().numbers["constraint_0_power_limit_uw"] == 55000000,
                "actual upper release clamp failed");
    } else if (name == "write_failure") {
        r.hardware().numbers["constraint_0_power_limit_uw"] = 55000000;
        r.hardware().fail_key = "constraint_0_power_limit_uw";
        r.hardware().failure = -EIO;
        r.set_curr_state(40000000, 1);
        require(r.hardware().writes.back().status == -EIO, "write error not observed");
        require(r.hardware().numbers["constraint_0_power_limit_uw"] == 55000000,
                "failed write changed actual hardware double");
        // Existing upstream caches the requested state even on error; a CLI
        // must use independent readback rather than trust this cache.
        require(r.get_curr_state() == 40000000, "upstream failure-cache behavior changed");
        r.hardware().fail_key.clear();
        r.set_curr_state(40000000, 1);
        require(r.hardware().numbers["constraint_0_power_limit_uw"] == 40000000,
                "retry after transient failure did not succeed");
    } else if (name == "bios_locked") {
        r.hardware().fail_key = "constraint_0_power_limit_uw";
        r.hardware().failure = -ENODATA;
        r.set_curr_state(40000000, 1);
        require(r.is_bios_locked(), "BIOS lock not detected");
        const auto writes = r.hardware().writes.size();
        r.set_curr_state(55000000, 0);
        require(r.hardware().writes.size() == writes, "BIOS-locked controller attempted write");
    } else if (name == "initially_disabled") {
        r.set_curr_state(40000000, 1);
        require(r.hardware().numbers["enabled"] == 1, "activation did not enable RAPL");
        r.set_curr_state(55000000, 0);
        require(r.hardware().numbers["enabled"] == 0,
                "counterexample: initially-disabled device loses persistent ceiling");
    } else if (name == "sequential_next_device" || name == "sequential_min_only_counterexample") {
        r.hardware().numbers["constraint_0_power_limit_uw"] = 55000000;
        // The valid-max, no-PPCC initialization above uses actual cached
        // controller state; the power-meter optimization remains disabled.
        next_cdev next;
        cthd_trip_point trip(0, PASSIVE, 90000, 1000, 0, DEFAULT_SENSOR_ID, SEQUENTIAL);
        trip.thd_trip_point_add_cdev(r, 10);
        trip.thd_trip_point_add_cdev(next, 0);
        bool reset = false;
        for (int i = 0; i < 8 && !r.in_max_state(); ++i)
            require(trip.thd_trip_point_check(DEFAULT_SENSOR_ID, 95000,
                    PREF_ENERGY_CONSERVE, &reset), "actual trip rejected active temperature");
        if (name == "sequential_min_only_counterexample") {
            require(!r.in_max_state() && r.get_curr_state() == 55000000,
                    "counterexample: invalid range must remain stuck at 55 W");
            require(next.writes == 0,
                    "counterexample: invalid range must block next cooling device");
            return 0;
        }
        require(r.in_max_state(), "paired range never reaches most restrictive state");
        require(r.get_curr_state() == 27500000, "actual controller did not reach configured floor");
        require(next.writes == 0, "sequential trip activated next device too early");
        require(trip.thd_trip_point_check(DEFAULT_SENSOR_ID, 95000,
                PREF_ENERGY_CONSERVE, &reset), "actual trip rejected next stage");
        require(next.writes == 1 && next.get_curr_state() > 0,
                "actual sequential trip failed to escalate next cooling device");
    } else {
        require(false, "unknown case");
    }
    return 0;
}
