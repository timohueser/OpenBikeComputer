import csv
import io
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import zipfile

from tools import planner_trains as trains


class RailProbeTests(unittest.TestCase):
    def test_rail_filter_preserves_timetable_and_transfer_references(self):
        tables = {
            'agency.txt': 'agency_id,agency_name,agency_url,agency_timezone\nr,Rail,https://rail.example,Europe/Berlin\nb,Bus,https://bus.example,Europe/Berlin\n',
            'routes.txt': 'route_id,agency_id,route_type\nr,r,2\ne,r,102\nb,b,700\n',
            'trips.txt': 'route_id,service_id,trip_id,shape_id,bikes_allowed\nr,w,t,s,1\ne,d,u,s,0\nb,w,b,s,2\n',
            'stop_times.txt': 'trip_id,stop_id,stop_sequence,arrival_time,departure_time,pickup_type,drop_off_type\nt,p,1,25:10:00,25:12:00,0,1\nu,q,1,26:00:00,26:01:00,1,0\nb,b,1,10:00:00,10:00:00,0,0\n',
            'stops.txt': 'stop_id,parent_station,location_type\np,s,0\ns,,1\nq,s,0\nb,,0\n',
            'calendar.txt': 'service_id,start_date,end_date\nw,20261001,20261031\nx,20261001,20261130\n',
            'calendar_dates.txt': 'service_id,date,exception_type\nw,20261004,2\nd,20261004,1\nx,20261004,1\n',
            'transfers.txt': 'from_stop_id,to_stop_id,transfer_type,min_transfer_time,from_route_id,to_trip_id\np,q,2,300,r,u\nq,p,3,,e,t\np,q,2,600,r,b\np,b,0,,,\n',
            'frequencies.txt': 'trip_id,start_time,end_time,headway_secs,exact_times\nu,26:00:00,27:00:00,600,1\nb,10:00:00,11:00:00,300,0\n',
            'attributions.txt': 'attribution_id,route_id,trip_id,organization_name\nfeed,,,Publisher\nrail,r,,Rail\nbus,b,,Bus\n',
            'shapes.txt': 'shape_id,shape_pt_sequence\ns,1\n',
        }
        with tempfile.TemporaryDirectory() as tmp:
            source, target = Path(tmp) / 'mixed.zip', Path(tmp) / 'rail.zip'
            with zipfile.ZipFile(source, 'w') as archive:
                for name, text in tables.items():
                    archive.writestr(name, text)
            report = trains.prepare(source, target)
            with zipfile.ZipFile(target) as archive:
                output = {name: list(trains.rows(archive, name)) for name in archive.namelist()}
            self.assertEqual({r['route_id'] for r in output['routes.txt']}, {'r', 'e'})
            self.assertEqual({r['stop_id'] for r in output['stops.txt']}, {'p', 'q', 's'})
            self.assertEqual(output['stop_times.txt'], list(csv.DictReader(io.StringIO(tables['stop_times.txt'])))[:2])
            self.assertEqual([r['exception_type'] for r in output['calendar_dates.txt']], ['2', '1'])
            self.assertEqual([r['transfer_type'] for r in output['transfers.txt']], ['2', '3'])
            self.assertEqual(output['transfers.txt'][0]['min_transfer_time'], '300')
            self.assertEqual([r['trip_id'] for r in output['frequencies.txt']], ['u'])
            self.assertEqual([r['attribution_id'] for r in output['attributions.txt']], ['feed', 'rail'])
            self.assertNotIn('shape_id', output['trips.txt'][0])
            self.assertEqual(report['bikes_allowed_trip_rows'], {'1': 1, '0': 1})
            self.assertEqual(report['omitted_tables'], ['shapes.txt'])
            with self.assertRaises(FileExistsError):
                trains.prepare(source, target)

    def test_departure_window_keeps_later_faster_witness_and_excludes_end(self):
        case = {'station': 's', 'start': '2026-10-04T08:00:00+02:00',
                'end': '2026-10-04T10:00:00+02:00', 'max_minutes': 90, 'changes': 1}

        def get(_url, endpoint, params):
            if endpoint == 'stoptimes':
                return {'stopTimes': [{'place': {'departure': t}} for t in (
                    '2026-10-04T06:30:00Z', '2026-10-04T07:30:00Z', '2026-10-04T08:00:00Z')]}
            duration = 80 if '06:30' in params['time'] else 20
            return {'all': [{'place': {'stopId': 'd'}, 'duration': duration, 'k': 2}]}

        with patch.object(trains, 'get', side_effect=get):
            result = trains.departure_window('http://localhost', case)
        self.assertEqual(result['departure_minutes'], 2)
        self.assertEqual(result['reachable']['d'], {
            'duration': 20, 'boardings': 2, 'query_start': '2026-10-04T07:30:00+00:00'})

    def test_departure_window_rejects_incomplete_paging(self):
        case = {'station': 's', 'start': '2026-10-04T08:00:00Z',
                'end': '2026-10-04T10:00:00Z', 'max_minutes': 90, 'changes': 1}
        with patch.object(trains, 'get', return_value={
                'stopTimes': [{'place': {'departure': '2026-10-04T08:30:00Z'}}]}):
            with self.assertRaisesRegex(ValueError, 'paging'):
                trains.departure_window('http://localhost', case)


if __name__ == '__main__':
    unittest.main()
