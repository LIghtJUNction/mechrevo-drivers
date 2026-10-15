/*
 * thd_zone.cpp: thermal zone class implementation
 *
 * Copyright (C) 2012 Intel Corporation. All rights reserved.
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
void cthd_zone::add_trip(cthd_trip_point &trip, int force) {
	if (force) {
		trip_points.push_back(trip);
		sort_and_update_poll_trip();
		return;
	}

	bool add = true;
	for (unsigned int j = 0; j < trip_points.size(); ++j) {
		if (trip_points[j].get_trip_type() == trip.get_trip_type()) {
			thd_log_debug("updating existing trip temp\n");
			trip_points[j] = trip;
			add = false;
			break;
		}
	}
	if (add)
		trip_points.push_back(trip);

	sort_and_update_poll_trip();
}

void cthd_zone::update_trip_temp(cthd_trip_point &trip) {
	for (unsigned int j = 0; j < trip_points.size(); ++j) {
		if (trip_points[j].get_trip_type() == trip.get_trip_type()) {
			thd_log_debug("updating existing trip temp\n");
			trip_points[j].update_trip_temp(trip.get_trip_temp());
			trip_points[j].update_trip_hyst(trip.get_trip_hyst());
			break;
		}
	}
	sort_and_update_poll_trip();
}
