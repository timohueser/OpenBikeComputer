import pygame
import sys
import os
import math
import time
from obcm.reader import OBCMReader
from obcm.viewport import Viewport

SCREEN_WIDTH = 1024
SCREEN_HEIGHT = 768

def get_distance(lon1, lat1, lon2, lat2):
    # Haversine distance in meters
    R = 6371000
    phi1, phi2 = math.radians(lat1 / 1e6), math.radians(lat2 / 1e6)
    dphi = math.radians((lat2 - lat1) / 1e6)
    dlambda = math.radians((lon2 - lon1) / 1e6)
    a = math.sin(dphi / 2)**2 + math.cos(phi1) * math.cos(phi2) * math.sin(dlambda / 2)**2
    c = 2 * math.atan2(math.sqrt(a), math.sqrt(1 - a))
    return R * c

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

    def handle_zoom(factor, vp):
        mouse_x, mouse_y = pygame.mouse.get_pos()
        old_lon, old_lat = vp.to_map(mouse_x, mouse_y)
        vp.zoom *= factor
        new_lon, new_lat = vp.to_map(mouse_x, mouse_y)
        vp.camera_lon += (old_lon - new_lon)
        vp.camera_lat += (old_lat - new_lat)

    with open(map_path, "rb") as f:
        reader = OBCMReader(f)
    
        # Init viewport at center of map
        min_lon, min_lat, max_lon, max_lat = reader.global_bbox
        vp = Viewport(SCREEN_WIDTH, SCREEN_HEIGHT, (min_lat + max_lat) // 2)
        vp.camera_lon = (min_lon + max_lon) // 2
        vp.camera_lat = (min_lat + max_lat) // 2
        
        # Initial zoom: fit map to screen width
        vp.zoom = SCREEN_WIDTH / (max_lon - min_lon) if max_lon != min_lon else 1.0
        
        debug_settings = {
            "show_bboxes": False,
            "show_perf": False
        }

        perf_metrics = {
            "query_ms": 0.0,
            "render_ms": 0.0
        }

        pygame.font.init()
        font = pygame.font.SysFont("monospace", 20)
        
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
                    elif event.button == 4: # Scroll up (legacy)
                        handle_zoom(1.2, vp)
                    elif event.button == 5: # Scroll down (legacy)
                        handle_zoom(1/1.2, vp)
                elif event.type == pygame.MOUSEWHEEL:
                    if event.y > 0: # Scroll up
                        handle_zoom(1.2, vp)
                    elif event.y < 0: # Scroll down
                        handle_zoom(1/1.2, vp)
                elif event.type == pygame.MOUSEBUTTONUP:
                    if event.button == 1:
                        panning = False
                elif event.type == pygame.MOUSEMOTION and panning:
                    dx, dy = event.rel
                    # Convert screen delta to map delta
                    vp.camera_lon -= dx / (vp.zoom * vp.aspect)
                    vp.camera_lat += dy / vp.zoom
                elif event.type == pygame.KEYDOWN:
                    if event.key == pygame.K_b:
                        debug_settings["show_bboxes"] = not debug_settings["show_bboxes"]
                        print(f"Debug: show_bboxes = {debug_settings['show_bboxes']}")
                    elif event.key == pygame.K_t:
                        debug_settings["show_perf"] = not debug_settings["show_perf"]
                        print(f"Debug: show_perf = {debug_settings['show_perf']}")

            bg_color = (30, 30, 30)
            if 99 in reader.styles:
                sea_color = reader.styles[99]["color"]
                r = (sea_color >> 11) & 0x1F
                g = (sea_color >> 5) & 0x3F
                b = sea_color & 0x1F
                bg_color = (r << 3, g << 2, b << 3)
                
            screen.fill(bg_color)
            
            # Only re-query if viewport changed
            current_vp_state = (vp.camera_lon, vp.camera_lat, vp.zoom)
            if current_vp_state != last_vp_state:
                t0 = time.perf_counter()
                last_vp_state = current_vp_state
                # Calculate visible BBox
                v_min_lon, v_max_lat = vp.to_map(0, 0)
                v_max_lon, v_min_lat = vp.to_map(SCREEN_WIDTH, SCREEN_HEIGHT)
                
                # Query visible chunks
                chunks = reader.query_bbox((v_min_lon, v_min_lat, v_max_lon, v_max_lat))
                visible_features = []
                for cid, node_bbox in chunks:
                    visible_features.extend(reader.decode_chunk(cid, node_bbox))
                
                # Sort by z_index
                visible_features.sort(key=lambda f: reader.styles.get(f["style_id"], {}).get("z_index", 0))
                
                perf_metrics["query_ms"] = (time.perf_counter() - t0) * 1000.0
            
            t_render_start = time.perf_counter()
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
                
                if f.get("type") == "polygon":
                    # Project exterior
                    ext_pts = [vp.to_screen(lon, lat) for lon, lat in f["exterior"]]
                    if len(ext_pts) >= 3:
                        pygame.draw.polygon(screen, rgb, ext_pts)
                    
                    # Workaround for holes: draw them over the polygon with the background color
                    if "interiors" in f:
                        for interior in f["interiors"]:
                            int_pts = [vp.to_screen(lon, lat) for lon, lat in interior]
                            if len(int_pts) >= 3:
                                pygame.draw.polygon(screen, bg_color, int_pts)
                else:
                    # Line
                    line_pts = [vp.to_screen(lon, lat) for lon, lat in f.get("points", [])]
                    if len(line_pts) >= 2:
                        pygame.draw.lines(screen, rgb, False, line_pts, max(1, style["weight"]))
            
            # --- Viewport Dimension Overlay ---
            v_min_lon, v_min_lat = vp.to_map(0, SCREEN_HEIGHT)
            v_max_lon, v_max_lat = vp.to_map(SCREEN_WIDTH, 0)
            
            width_m = get_distance(v_min_lon, v_min_lat, v_max_lon, v_min_lat)
            height_m = get_distance(v_min_lon, v_min_lat, v_min_lon, v_max_lat)
            
            def format_dist(meters):
                if meters > 1000:
                    return f"{meters / 1000:.2f} km"
                return f"{int(meters)} m"

            dim_text = f"{format_dist(width_m)} x {format_dist(height_m)}"
            dim_surf = font.render(dim_text, True, (255, 255, 255))
            
            bg_rect = dim_surf.get_rect()
            bg_rect.bottomleft = (10, SCREEN_HEIGHT - 10)
            
            bg_surface = pygame.Surface((bg_rect.width + 10, bg_rect.height + 5), pygame.SRCALPHA)
            bg_surface.fill((0, 0, 0, 150))
            screen.blit(bg_surface, bg_rect.inflate(10, 5))
            screen.blit(dim_surf, bg_rect)

            # Debug Overlay
            if debug_settings["show_bboxes"]:
                for cid, node_bbox in chunks:
                    # node_bbox: (min_lon, min_lat, max_lon, max_lat)
                    min_lon, min_lat, max_lon, max_lat = node_bbox
                    
                    # Project corners
                    # Top-Left, Top-Right, Bottom-Right, Bottom-Left
                    points = [
                        vp.to_screen(min_lon, max_lat),
                        vp.to_screen(max_lon, max_lat),
                        vp.to_screen(max_lon, min_lat),
                        vp.to_screen(min_lon, min_lat)
                    ]
                    
                    # Draw bright green rectangle (0, 255, 0)
                    pygame.draw.lines(screen, (0, 255, 0), True, points, 1)
            
            perf_metrics["render_ms"] = (time.perf_counter() - t_render_start) * 1000.0

            # Performance Overlay
            if debug_settings["show_perf"]:
                labels = [
                    f"Query:  {perf_metrics['query_ms']:.2f} ms",
                    f"Render: {perf_metrics['render_ms']:.2f} ms"
                ]
                
                for i, text in enumerate(labels):
                    surf = font.render(text, True, (255, 255, 255))
                    # Background rect for readability
                    bg_rect = surf.get_rect()
                    bg_rect.bottomright = (SCREEN_WIDTH - 10, SCREEN_HEIGHT - 10 - (i * 25))
                    
                    # Draw semi-transparent background
                    bg_surface = pygame.Surface((bg_rect.width + 10, bg_rect.height + 5), pygame.SRCALPHA)
                    bg_surface.fill((0, 0, 0, 150))
                    screen.blit(bg_surface, bg_rect.inflate(10, 5))
                    
                    screen.blit(surf, bg_rect)
            
            pygame.display.flip()
            clock.tick(60)

if __name__ == "__main__":
    main()
