/********************************************************************************
 * Copyright (c) 2026 Contributors to the Eclipse Foundation
 *
 * See the NOTICE file(s) distributed with this work for additional
 * information regarding copyright ownership.
 *
 * This program and the accompanying materials are made available under the
 * terms of the Apache License Version 2.0 which is available at
 * https://www.apache.org/licenses/LICENSE-2.0
 *
 * SPDX-License-Identifier: Apache-2.0
 ********************************************************************************/
#include "score/opensovd-core/bridge/read_did.h"

#include <gtest/gtest.h>

#include <chrono>
#include <condition_variable>
#include <memory>
#include <mutex>
#include <vector>

namespace
{
using score::mw::diag::ByteVector;
using score::mw::diag::Future;
using score::mw::diag::uds::MetaData;
using score::mw::diag::uds::ReadDataByIdentifier;
using score::mw::diag::uds::Result;

/// Outcome of one read. The completion callback and the test share it, so a timed-out test can return while a
/// late callback is still pending without leaving the callback a dangling reference.
class Completion
{
  public:
    void Record(std::uint8_t status, const std::uint8_t* bytes, std::size_t size)
    {
        std::lock_guard<std::mutex> guard{mutex_};
        status_ = status;
        if (size != 0U)
        {
            bytes_.assign(bytes, bytes + size);
        }
        ++count_;
        changed_.notify_one();
    }

    bool Wait()
    {
        std::unique_lock<std::mutex> guard{mutex_};
        return changed_.wait_for(guard, std::chrono::seconds{2}, [this]() {
            return count_ != 0U;
        });
    }

    std::size_t Count()
    {
        std::lock_guard<std::mutex> guard{mutex_};
        return count_;
    }

    std::uint8_t Status()
    {
        std::lock_guard<std::mutex> guard{mutex_};
        return status_;
    }

    std::vector<std::uint8_t> Bytes()
    {
        std::lock_guard<std::mutex> guard{mutex_};
        return bytes_;
    }

  private:
    std::mutex mutex_;
    std::condition_variable changed_;
    std::size_t count_{0U};
    std::uint8_t status_{SCORE_DIAG_READ_FAILED};
    std::vector<std::uint8_t> bytes_;
};

using Pending = std::shared_ptr<Completion>;
using ReaderHandle = std::unique_ptr<ScoreDiagReader, void (*)(ScoreDiagReader*)>;
using RequestHandle = std::unique_ptr<ScoreDiagReadRequest, void (*)(ScoreDiagReadRequest*)>;

void Complete(void* context, std::uint8_t status, std::uint8_t, const std::uint8_t* bytes, std::size_t size)
{
    std::unique_ptr<Pending> owned{static_cast<Pending*>(context)};
    (*owned)->Record(status, bytes, size);
}

ReaderHandle Register(const std::shared_ptr<ReadDataByIdentifier>& handler)
{
    auto reader = score::opensovd::bridge::RegisterReadDataByIdentifier(handler);
    EXPECT_TRUE(reader.has_value());
    return ReaderHandle{reader.has_value() ? reader.value() : nullptr, score_diag_reader_release};
}

/// Starts a read. The completion context is reclaimed here when the read does not start, because the bridge
/// then never calls the completion.
ScoreDiagStatus TryStartRead(ScoreDiagReader* reader, const Pending& completion, ScoreDiagReadRequest** request)
{
    auto* context = new Pending{completion};
    const auto status = score_diag_read_start(reader, context, Complete, request);
    if (status != SCORE_DIAG_OK)
    {
        delete context;
    }
    return status;
}

RequestHandle StartRead(ScoreDiagReader* reader, const Pending& completion)
{
    ScoreDiagReadRequest* request = nullptr;
    EXPECT_EQ(TryStartRead(reader, completion, &request), SCORE_DIAG_OK);
    return RequestHandle{request, score_diag_read_release};
}

class DeferredReader final : public ReadDataByIdentifier
{
  public:
    Future<Result<ByteVector>> Read(const MetaData& metadata, score::cpp::stop_token stop_token) override
    {
        std::lock_guard<std::mutex> guard{mutex};
        received_metadata = metadata;
        received_token = stop_token;
        invoked = true;
        changed.notify_one();
        return promise.GetInterruptibleFuture().value();
    }

    bool WaitForInvocation()
    {
        std::unique_lock<std::mutex> guard{mutex};
        return changed.wait_for(guard, std::chrono::seconds{2}, [this]() {
            return invoked;
        });
    }

    score::mw::diag::Promise<Result<ByteVector>> promise;
    MetaData received_metadata;
    score::cpp::stop_token received_token;

  private:
    std::mutex mutex;
    std::condition_variable changed;
    bool invoked{false};
};

class GatedReader final : public ReadDataByIdentifier
{
  public:
    Future<Result<ByteVector>> Read(const MetaData&, score::cpp::stop_token) override
    {
        std::unique_lock<std::mutex> guard{mutex};
        ++invocations;
        changed.notify_all();
        changed.wait(guard, [this]() {
            return open;
        });
        return promise.GetInterruptibleFuture().value();
    }

    bool WaitForInvocations(std::size_t count)
    {
        std::unique_lock<std::mutex> guard{mutex};
        return changed.wait_for(guard, std::chrono::seconds{2}, [this, count]() {
            return invocations >= count;
        });
    }

    void Open()
    {
        std::lock_guard<std::mutex> guard{mutex};
        open = true;
        changed.notify_all();
    }

    std::size_t Invocations()
    {
        std::lock_guard<std::mutex> guard{mutex};
        return invocations;
    }

    score::mw::diag::Promise<Result<ByteVector>> promise;

  private:
    std::mutex mutex;
    std::condition_variable changed;
    std::size_t invocations{0U};
    bool open{false};
};

TEST(ReadDidBridge, PendingReadSurvivesRegistrationRelease)
{
    RecordProperty("PartiallyVerifies", "comp_req__uds_data_bridge__read_handlers");
    RecordProperty("Description",
                   "Check that a read started on a registered ReadDataByIdentifier handler completes with its data "
                   "after the registration is released.");
    RecordProperty("TestType", "requirements-based");
    RecordProperty("DerivationTechnique", "requirements-analysis");

    auto handler = std::make_shared<DeferredReader>();
    auto reader = Register(handler);
    auto reply = std::make_shared<Completion>();
    auto request = StartRead(reader.get(), reply);
    ASSERT_NE(request.get(), nullptr);
    ASSERT_TRUE(handler->WaitForInvocation());
    reader.reset();
    EXPECT_EQ(handler->received_metadata.session, score::mw::diag::uds::DiagnosticSession::Default);
    EXPECT_FALSE(handler->received_metadata.security_level.has_value());
    EXPECT_EQ(handler->received_metadata.addressing, score::mw::diag::uds::AddressingMode::Physical);
    EXPECT_FALSE(handler->received_metadata.source_address.has_value());
    EXPECT_FALSE(handler->received_metadata.target_address.has_value());
    EXPECT_TRUE(
        handler->promise.SetValue(Result<ByteVector>{ByteVector{std::byte{0x00}, std::byte{0xff}}}).has_value());
    EXPECT_TRUE(reply->Wait());
    EXPECT_EQ(reply->Status(), SCORE_DIAG_READ_OK);
    EXPECT_EQ(reply->Count(), 1U);
    EXPECT_EQ(reply->Bytes(), (std::vector<std::uint8_t>{0x00, 0xff}));
}

TEST(ReadDidBridge, CancellationCompletesOnceAndStopsHandlerToken)
{
    RecordProperty("PartiallyVerifies", "comp_req__uds_data_bridge__read_timeout");
    RecordProperty(
        "Description",
        "Check that cancelling a pending read completes it once as cancelled and requests the handler to stop.");
    RecordProperty("TestType", "requirements-based");
    RecordProperty("DerivationTechnique", "requirements-analysis");

    auto handler = std::make_shared<DeferredReader>();
    auto reader = Register(handler);
    auto reply = std::make_shared<Completion>();
    auto request = StartRead(reader.get(), reply);
    ASSERT_NE(request.get(), nullptr);
    ASSERT_TRUE(handler->WaitForInvocation());
    score_diag_read_cancel(request.get());
    EXPECT_TRUE(reply->Wait());
    EXPECT_EQ(reply->Status(), SCORE_DIAG_READ_CANCELLED);
    EXPECT_TRUE(handler->received_token.stop_requested());
    EXPECT_TRUE(handler->promise.SetValue(Result<ByteVector>{ByteVector{std::byte{0x12}}}).has_value());
    EXPECT_EQ(reply->Count(), 1U);
}

TEST(ReadDidBridge, ReadCancelledWhileQueuedIsNotDispatched)
{
    RecordProperty("PartiallyVerifies", "comp_req__uds_data_bridge__read_timeout");
    RecordProperty("Description",
                   "Check that a read cancelled while it waits for the dispatch of another read is never passed to "
                   "the handler.");
    RecordProperty("TestType", "requirements-based");
    RecordProperty("DerivationTechnique", "requirements-analysis");

    auto handler = std::make_shared<GatedReader>();
    auto reader = Register(handler);
    auto first_reply = std::make_shared<Completion>();
    auto queued_reply = std::make_shared<Completion>();
    auto first = StartRead(reader.get(), first_reply);
    ASSERT_NE(first.get(), nullptr);
    ASSERT_TRUE(handler->WaitForInvocations(1U));
    auto queued = StartRead(reader.get(), queued_reply);
    ASSERT_NE(queued.get(), nullptr);
    score_diag_read_cancel(queued.get());
    handler->Open();
    EXPECT_TRUE(queued_reply->Wait());
    EXPECT_EQ(queued_reply->Status(), SCORE_DIAG_READ_CANCELLED);
    EXPECT_TRUE(handler->promise.SetValue(Result<ByteVector>{ByteVector{std::byte{0x12}}}).has_value());
    EXPECT_TRUE(first_reply->Wait());
    EXPECT_EQ(first_reply->Status(), SCORE_DIAG_READ_OK);
    EXPECT_EQ(handler->Invocations(), 1U);
}

TEST(ReadDidBridge, CompletedReadsDoNotHoldAdmissionUntilReleased)
{
    RecordProperty("PartiallyVerifies", "comp_req__uds_data_bridge__read_admission");
    RecordProperty("Description",
                   "Check that completed reads keep no admission slot, so that a ninth read starts after eight "
                   "completed reads whose handles are still held.");
    RecordProperty("TestType", "requirements-based");
    RecordProperty("DerivationTechnique", "requirements-analysis");

    ScoreDiagReader* raw_reader = nullptr;
    ASSERT_EQ(score_diag_demo_reader_create(0U, 0U, &raw_reader), SCORE_DIAG_OK);
    ReaderHandle reader{raw_reader, score_diag_reader_release};
    ASSERT_NE(reader.get(), nullptr);
    std::vector<Pending> replies;
    std::vector<RequestHandle> requests;
    for (std::size_t index = 0U; index < 8U; ++index)
    {
        replies.push_back(std::make_shared<Completion>());
        requests.push_back(StartRead(reader.get(), replies.back()));
        ASSERT_NE(requests.back().get(), nullptr);
        EXPECT_TRUE(replies[index]->Wait());
        EXPECT_EQ(replies[index]->Status(), SCORE_DIAG_READ_OK);
    }
    replies.push_back(std::make_shared<Completion>());
    requests.push_back(StartRead(reader.get(), replies.back()));
    EXPECT_NE(requests.back().get(), nullptr);
    EXPECT_TRUE(replies[8U]->Wait());
    EXPECT_EQ(replies[8U]->Status(), SCORE_DIAG_READ_OK);
}

TEST(ReadDidBridge, RegistrationsOfOneHandlerShareAdmission)
{
    RecordProperty("PartiallyVerifies", "comp_req__uds_data_bridge__read_admission");
    RecordProperty("Description",
                   "Check that reads through every registration of one handler count against the same admission "
                   "limit, so that a ninth read through a second registration is rejected as busy.");
    RecordProperty("TestType", "requirements-based");
    RecordProperty("DerivationTechnique", "requirements-analysis");

    auto handler = std::make_shared<GatedReader>();
    auto first = Register(handler);
    auto second = Register(handler);
    std::vector<Pending> replies;
    std::vector<RequestHandle> requests;
    for (std::size_t index = 0U; index < 8U; ++index)
    {
        replies.push_back(std::make_shared<Completion>());
        requests.push_back(StartRead(first.get(), replies.back()));
        ASSERT_NE(requests.back().get(), nullptr);
    }
    ASSERT_TRUE(handler->WaitForInvocations(1U));
    ScoreDiagReadRequest* ninth = nullptr;
    EXPECT_EQ(TryStartRead(second.get(), std::make_shared<Completion>(), &ninth), SCORE_DIAG_BUSY);
    EXPECT_EQ(ninth, nullptr);
    for (auto& request : requests)
    {
        score_diag_read_cancel(request.get());
    }
    handler->Open();
    for (auto& reply : replies)
    {
        EXPECT_TRUE(reply->Wait());
        EXPECT_EQ(reply->Status(), SCORE_DIAG_READ_CANCELLED);
    }
    EXPECT_EQ(handler->Invocations(), 1U);
}
}  // namespace
