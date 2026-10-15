/*
 * cthd_engine_defualt.cpp: Default thermal engine
 *
 * Copyright (C) 2013 Intel Corporation. All rights reserved.
 *
 * This program is free software; you can redistribute it and/or
 * modify it under the terms of the GNU General Public License version
 * 2 or later as published by the Free Software Foundation.
 *
 * This program is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 * GNU General Public License for more details.
 *
 * You should have received a copy of the GNU General Public License
 * along with this program; if not, write to the Free Software
 * Foundation, Inc., 51 Franklin Street, Fifth Floor, Boston, MA
 * 02110-1301, USA.
 *
 *
 * Author Name <Srinivas.Pandruvada@linux.intel.com>
 *
 */
					cthd_sensor *sensor = search_sensor(
							trip_pt_config.sensor_type);
					if (!sensor) {
						thd_log_error("XML zone: invalid sensor type [%s]\n",
								trip_pt_config.sensor_type.c_str());
						// This will update the trip temperature for the matching
						// trip type
						if (trip_pt_config.temperature) {
							cthd_trip_point trip_pt(zone->get_trip_count(),
									trip_pt_config.trip_pt_type,
									trip_pt_config.temperature,
									trip_pt_config.hyst, zone->get_zone_index(),
									-1, trip_pt_config.control_type);
							zone->update_trip_temp(trip_pt);
						}
						continue;
					}
					zone->bind_sensor(sensor);
					if (trip_pt_config.temperature) {

						cthd_trip_point trip_pt(zone->get_trip_count(),
								trip_pt_config.trip_pt_type,
								trip_pt_config.temperature, trip_pt_config.hyst,
								zone->get_zone_index(), sensor->get_index(),
								trip_pt_config.control_type);

						if (trip_pt_config.dependency.dependency) {
							trip_pt.set_dependency(trip_pt_config.dependency.cdev, trip_pt_config.dependency.state);
						}

						// bind cdev
						for (unsigned int j = 0;
								j < trip_pt_config.cdev_trips.size(); ++j) {
							cthd_cdev *cdev = search_cdev(
									trip_pt_config.cdev_trips[j].type);
							if (cdev) {
								trip_pt.thd_trip_point_add_cdev(*cdev,
										trip_pt_config.cdev_trips[j].influence,
										trip_pt_config.cdev_trips[j].sampling_period,
										trip_pt_config.cdev_trips[j].target_state_valid,
										trip_pt_config.cdev_trips[j].target_state,
									&trip_pt_config.cdev_trips[j].pid_param,
									trip_pt_config.cdev_trips[j].min_max_valid,
									trip_pt_config.cdev_trips[j].target_min_state,
									trip_pt_config.cdev_trips[j].target_max_state);
								zone->zone_cdev_set_binded();
								activate = true;
							}
						}
						zone->add_trip(trip_pt);
					}
