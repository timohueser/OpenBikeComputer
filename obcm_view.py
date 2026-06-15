import pygame
import sys
import os
from obcm.reader import OBCMReader
from obcm.viewport import Viewport

SCREEN_WIDTH = 1024
SCREEN_HEIGHT = 768

def main():
    if len(sys.argv) < 2:
        print("Usage: python obcm_view.py <map.obcm>")
        return

    map_path = sys.argv[1]
    if not os.path.exists(map_path):
        print(f"Error: File not found: {map_path}")
        return

    pygame.init()
    screen = pygame.display.set_mode((SCREEN_WIDTH, SCREEN_HEIGHT))
    pygame.display.set_caption(f"OBCM Visualizer - {os.path.basename(map_path)}")
    clock = pygame.time.Clock()

    with open(map_path, "rb") as f:
        reader = OBCMReader(f)
    
        # Init viewport at center of map
        min_lon, min_lat, max_lon, max_lat = reader.global_bbox
        vp = Viewport(SCREEN_WIDTH, SCREEN_HEIGHT, (min_lat + max_lat) // 2)
        vp.camera_lon = (min_lon + max_lon) // 2
        vp.camera_lat = (min_lat + max_lat) // 2
        
        # Initial zoom: fit map to screen width
        vp.zoom = SCREEN_WIDTH / (max_lon - min_lon) if max_lon != min_lon else 1.0
        
        panning = False
        
        last_vp_state = None
        visible_features = []
        
        while True:
            for event in pygame.event.get():
                if event.type == pygame.QUIT:
                    pygame.quit()
                    sys.exit()
                elif event.type == pygame.MOUSEBUTTONDOWN:
                    if event.button == 1: # Left click
                        panning = True
                    elif event.button == 4: # Scroll up
                        vp.zoom *= 1.2
                    elif event.button == 5: # Scroll down
                        vp.zoom /= 1.2
                elif event.type == pygame.MOUSEBUTTONUP:
                    if event.button == 1:
                        panning = False
                elif event.type == pygame.MOUSEMOTION and panning:
                    dx, dy = event.rel
                    # Convert screen delta to map delta
                    vp.camera_lon -= dx / (vp.zoom * vp.aspect)
                    vp.camera_lat += dy / vp.zoom

            screen.fill((30, 30, 30))
            
            # Only re-query if viewport changed
            current_vp_state = (vp.camera_lon, vp.camera_lat, vp.zoom)
            if current_vp_state != last_vp_state:
                last_vp_state = current_vp_state
                # Calculate visible BBox
                v_min_lon, v_max_lat = vp.to_map(0, 0)
                v_max_lon, v_min_lat = vp.to_map(SCREEN_WIDTH, SCREEN_HEIGHT)
                
                # Query visible chunks
                chunks = reader.query_bbox((v_min_lon, v_min_lat, v_max_lon, v_max_lat))
                visible_features = []
                for cid, node_bbox in chunks:
                    visible_features.extend(reader.decode_chunk(cid, node_bbox))
            
            # Draw visible features
            for f in visible_features:
                if f["style_id"] not in reader.styles:
                    continue
                style = reader.styles[f["style_id"]]
                
                # RGB565 to RGB888
                color = style["color"]
                r = (color >> 11) & 0x1F
                g = (color >> 5) & 0x3F
                b = color & 0x1F
                rgb = (r << 3, g << 2, b << 3)
                
                # Project points
                line_pts = [vp.to_screen(lon, lat) for lon, lat in f["points"]]
                if len(line_pts) >= 2:
                    pygame.draw.lines(screen, rgb, False, line_pts, max(1, style["weight"]))
            
            pygame.display.flip()
            clock.tick(60)

if __name__ == "__main__":
    main()
