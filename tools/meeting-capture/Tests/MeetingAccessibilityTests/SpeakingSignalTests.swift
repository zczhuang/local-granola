import CaptureCore
import MeetingAccessibility
import XCTest

// LOCAL-GRANOLA(mac-untested): fixtures follow the speaking labels the adapters assume for Zoom
// and Teams. Nobody has read them from a live call; re-record these on a Mac before trusting them.
final class SpeakingSignalTests: XCTestCase {
  func zoom(rows: [(name: String, state: String?)], header: String? = "Participants (3)")
    -> MeetingAXSnapshot
  {
    var nodes: [MeetingAXNode] = [
      .init(id: "window", role: "AXWindow", identifier: "zm.meeting.window.main"),
      .init(id: "audio", parentID: "window", role: "AXButton", identifier: "audio"),
      .init(id: "leave", parentID: "window", role: "AXButton", identifier: "leave"),
      .init(id: "participants", parentID: "window", role: "AXButton", identifier: "participant"),
      .init(id: "roster", parentID: "window", role: "AXOutline", description: "Participants list"),
    ]
    if let header {
      nodes.append(.init(id: "head", parentID: "window", role: "AXStaticText", title: header))
    }
    for (index, row) in rows.enumerated() {
      nodes.append(.init(id: "row\(index)", parentID: "roster", role: "AXRow"))
      nodes.append(
        .init(id: "name\(index)", parentID: "row\(index)", role: "AXStaticText", title: row.name))
      if let state = row.state {
        nodes.append(
          .init(id: "state\(index)", parentID: "row\(index)", role: "AXStaticText", title: state))
      }
    }
    return .init(nodes: nodes, complete: true)
  }

  func teams(rows: [(name: String, description: String)]) -> MeetingAXSnapshot {
    var nodes: [MeetingAXNode] = [
      .init(id: "page", role: "AXWebArea", url: "https://teams.live.com/v2/"),
      .init(id: "roster", parentID: "page", role: "AXOutline", description: "Attendees"),
      .init(
        id: "header", parentID: "roster", role: "AXRow",
        description: "In this meeting, \(rows.count) total"),
      .init(id: "controls", parentID: "page", role: "AXToolbar", description: "Meeting controls"),
      .init(id: "people", parentID: "controls", role: "AXButton", title: "People"),
      .init(id: "mic", parentID: "controls", role: "AXButton", title: "Unmute mic"),
      .init(id: "calling", parentID: "page", role: "AXGroup", description: "Calling controls"),
      .init(id: "leave", parentID: "calling", role: "AXButton", title: "Leave"),
    ]
    for (index, row) in rows.enumerated() {
      nodes.append(
        .init(id: "row\(index)", parentID: "roster", role: "AXRow", description: row.description))
      nodes.append(
        .init(id: "name\(index)", parentID: "row\(index)", role: "AXStaticText", title: row.name))
    }
    return .init(nodes: nodes, complete: true)
  }

  func meeting(_ bundleID: String, _ snapshot: MeetingAXSnapshot) throws -> MeetingObservation {
    guard
      case .meeting(let meeting) = MeetingAXAdapter.project(
        bundleID: bundleID, snapshot: snapshot)
    else {
      throw XCTSkip("not a meeting")
    }
    return meeting
  }

  func testZoomRowThatSaysTalkingIsTheSpeaker() throws {
    let result = try meeting(
      "us.zoom.xos",
      zoom(rows: [
        ("Alex (Host, me)", "Talking"), ("Sam", "Talking"), ("Jo", "Not talking"),
      ]))
    XCTAssertEqual(result.speakerStatus, "available")
    // The local user's own row says nothing about the call's voices.
    XCTAssertEqual(
      result.speakers,
      [.init(platform: "zoom", participantID: "window/name1", displayName: "Sam")])
    XCTAssertEqual(result.participantCount, 3)
  }

  func testZoomWithNoSpeakingStateIsUnavailableNotNobody() throws {
    let result = try meeting("us.zoom.xos", zoom(rows: [("Alex (me)", nil), ("Sam", nil)]))
    XCTAssertEqual(result.speakerStatus, "unavailable")
    XCTAssertEqual(result.speakers, [])
  }

  func testZoomWhenNobodyTalksIsAvailableAndEmpty() throws {
    let result = try meeting(
      "us.zoom.xos", zoom(rows: [("Alex (me)", "Not talking"), ("Sam", "Not talking")]))
    XCTAssertEqual(result.speakerStatus, "available")
    XCTAssertEqual(result.speakers, [])
  }

  func testZoomCountIsOnlyTakenFromOneUnambiguousHeader() throws {
    let none = try meeting("us.zoom.xos", zoom(rows: [("Sam", nil)], header: nil))
    XCTAssertNil(none.participantCount)
  }

  func testTeamsSpeakingRowNamesTheSpeaker() throws {
    let result = try meeting(
      "com.microsoft.teams2",
      teams(rows: [
        ("Alex (You)", "Alex (You), Organizer, Muted"),
        ("Sam", "Sam, Speaking"),
      ]))
    XCTAssertEqual(result.speakerStatus, "available")
    XCTAssertEqual(
      result.speakers,
      [.init(platform: "teams", participantID: "page/name1", displayName: "Sam")])
    XCTAssertEqual(result.participantCount, 2)
    XCTAssertEqual(result.participants[0].displayName, "Alex")
    XCTAssertEqual(result.participants[0].isLocal, true)
    XCTAssertNil(result.participants[1].isLocal)
  }

  func testTeamsMutedRowIsNeverSpeaking() throws {
    let result = try meeting(
      "com.microsoft.teams2",
      teams(rows: [("Alex", "Alex, Organizer, Unmuted"), ("Sam", "Sam, Muted")]))
    XCTAssertEqual(result.speakerStatus, "unavailable")
    XCTAssertEqual(result.speakers, [])
  }

  func testWireCarriesTheSpeakerForAnarlog() throws {
    let result = try meeting(
      "com.microsoft.teams2",
      teams(rows: [("Alex (You)", "Alex (You), Muted"), ("Sam", "Sam, Speaking")]))
    let line = try XCTUnwrap(EventLine.parse(EventLine.meetingObservation(result).text))
    XCTAssertEqual(line.value("speakerStatus"), "available")
    XCTAssertEqual(line.value("speaker.id"), "page/name1")
    XCTAssertEqual(line.value("speaker.name"), "Sam")
  }
}
