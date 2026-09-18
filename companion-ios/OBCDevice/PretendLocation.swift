#if DEBUG

    import CoreLocation
    import MapKit
    import SwiftUI

    /// A place the app pretends to stand at.
    struct PretendPlace {
        var name: String
        var coordinate: CLLocationCoordinate2D

        var text: String { String(format: "%.5f, %.5f", coordinate.latitude, coordinate.longitude) }
    }

    /// The developer sheet's location override: look a place up by name, or type a latitude and a
    /// longitude, and the host gets that position instead of the phone's own.
    struct PretendLocationSection: View {
        let controller: HostController
        @State private var query = ""
        @State private var results: [PretendPlace] = []
        @State private var isSearching = false
        @State private var latitude = ""
        @State private var longitude = ""

        var body: some View {
            Section("Pretend location") {
                if let place = controller.pretendPlace {
                    LabeledContent(place.name, value: place.text)
                        .foregroundStyle(.orange)
                    Button("Back to the real GPS", systemImage: "location") { controller.pretend(nil) }
                } else {
                    Text("The phone's own GPS").foregroundStyle(.secondary)
                }

                HStack {
                    TextField("Place name", text: $query)
                        .autocorrectionDisabled()
                        .textInputAutocapitalization(.never)
                        .onSubmit(search)
                    if isSearching { ProgressView() }
                }
                ForEach(results.indices, id: \.self) { index in
                    Button { apply(results[index]) } label: {
                        VStack(alignment: .leading) {
                            Text(results[index].name)
                            Text(results[index].text).font(.caption).foregroundStyle(.secondary)
                        }
                    }
                }

                HStack {
                    TextField("Latitude", text: $latitude).keyboardType(.numbersAndPunctuation)
                    TextField("Longitude", text: $longitude).keyboardType(.numbersAndPunctuation)
                    Button("Go") { if let typed { apply(typed) } }
                        .buttonStyle(.borderless)
                        .disabled(typed == nil)
                }
            }
        }

        /// The typed pair, when it is a real coordinate.
        private var typed: PretendPlace? {
            guard let lat = Double(latitude), let lon = Double(longitude) else { return nil }
            let coordinate = CLLocationCoordinate2D(latitude: lat, longitude: lon)
            guard CLLocationCoordinate2DIsValid(coordinate) else { return nil }
            return PretendPlace(name: "Typed", coordinate: coordinate)
        }

        /// The whole world, not the region around the phone: the point is to go far away.
        private func search() {
            let request = MKLocalSearch.Request()
            request.naturalLanguageQuery = query
            request.region = MKCoordinateRegion(.world)
            results = []
            isSearching = true
            Task {
                let response = try? await MKLocalSearch(request: request).start()
                results = (response?.mapItems ?? []).map {
                    PretendPlace(name: $0.name ?? query, coordinate: $0.placemark.coordinate)
                }
                isSearching = false
            }
        }

        private func apply(_ place: PretendPlace) {
            controller.pretend(place)
            results = []
        }
    }

#endif
