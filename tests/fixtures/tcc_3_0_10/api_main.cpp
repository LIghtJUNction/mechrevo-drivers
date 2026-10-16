// SPDX-License-Identifier: GPL-3.0-or-later
#include <cassert>
#include <cstdlib>
#include <iostream>
#include "tuxedo_io_lib/tuxedo_io_api.hh"
int main() {
 IO io(TUXEDO_IO_DEVICE_FILE);UniwillDevice device(io);int output=-777;
 setenv("TCC_MOCK_SUCCESS","0",1);setenv("TCC_MOCK_RAW","146",1);
 if(device.GetFanSpeedPercent(0,output) || output!=-777) return 10;
 output=-777;if(device.GetFanSpeedPercent(1,output) || output!=-777)return 11;
 output=-777;if(device.GetFanSpeedPercent(2,output) || output!=-777)return 12;
 setenv("TCC_MOCK_SUCCESS","1",1);
 for(int raw: {0,146,200}) {setenv("TCC_MOCK_RAW",std::to_string(raw).c_str(),1);output=-777;assert(device.GetFanSpeedPercent(0,output));assert(output==raw/2);}
 std::cout << "actual API: failed/invalid reads preserve output; success 0/73/100\n";
}
