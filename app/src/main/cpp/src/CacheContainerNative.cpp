/*
 * Copyright 2016 Luca Martino.
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copyFile of the License at
 *
 * http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

#include <jni.h>
#include <stdio.h>
#include <vector>
#include <android/log.h>
#include <string>
//#include "onnxruntime/core/session/onnxruntime_c_api.h"
//#include "onnxruntime/core/session/onnxruntime_c_api.h"

std::vector<int> jintArrayTointVector2(JNIEnv* env, jintArray jarray);


class CacheContainer{
private:
    float ** cacheContainer;
    int dim1;  //32*2
    int dim2;  //batch_size
    int dim3;  //num attention heads -- caller-provided as a starting guess only, see insertData()
    int dim4;  //sequence_length
    int dim5;  //128
    bool dim3Confirmed = false;  //whether dim3 has been validated/corrected against a real buffer yet

public:
    CacheContainer(int dim1, int dim2, int dim3, int dim4, int dim5){
        this->dim1 = dim1;  //32*2
        this->dim2 = dim2;  //batch_size
        this->dim3 = dim3;  //num attention heads (caller's guess, see insertData())
        this->dim4 = dim4;  //sequence_length
        this->dim5 = dim5;  //128
        cacheContainer = new float*[dim1];
    }

    // BUG FIX (see docs/beam-search-crash-fix.md): dim3 (attention head count) used to
    // arrive hardcoded from the Java call site (always 16, regardless of which model/mode
    // was active), and a mismatch against the tensor's real size was only logged, never
    // treated as fatal -- insertData() would store the pointer and let reorder() index into
    // it with the wrong dim3 anyway, reading/writing out of bounds relative to the actual
    // allocation. That's undefined behavior with exactly the "sometimes it crashes,
    // sometimes it doesn't" signature the removed beam-search path was disabled for. Fixed
    // by deriving the true head count from the first real buffer's own size instead of
    // trusting the caller, and by making a genuine, uncorrectable mismatch a thrown JNI
    // exception instead of silently continuing with corrupt state.
    void insertData(JNIEnv *env, int index, jobject dataBuffer){
        float * bufferArr = (float *)(*env).GetDirectBufferAddress(dataBuffer);
        long size = (*env).GetDirectBufferCapacity(dataBuffer);

        if (!dim3Confirmed) {
            long perHeadElements = (long) dim2 * dim4 * dim5;
            if (perHeadElements <= 0 || size <= 0 || size % (perHeadElements * (long) sizeof(float)) != 0) {
                // Not even a whole-number head count -- dim2/dim4/dim5 themselves must be
                // wrong (not just dim3's guess), which isn't something this class can
                // recover from on its own.
                jclass exceptionClass = env->FindClass("java/lang/IllegalStateException");
                if (exceptionClass != nullptr) {
                    env->ThrowNew(exceptionClass, "CacheContainerNative: buffer size is not a whole multiple of "
                                                   "batch*seq*headDim -- dim2/dim4/dim5 are wrong, not just the head count");
                }
                return;
            }
            int actualDim3 = (int) (size / (perHeadElements * (long) sizeof(float)));
            if (actualDim3 != dim3) {
                __android_log_print(ANDROID_LOG_WARN, "CACHE CONTAINER NATIVE",
                                     "head count passed from Java (%d) didn't match the actual tensor (%d heads) -- "
                                     "using the actual value instead of silently corrupting memory with the wrong one",
                                     dim3, actualDim3);
                dim3 = actualDim3;
            }
            dim3Confirmed = true;
        } else {
            // Every subsequent present.*.decoder.key/value tensor must share the same
            // shape as the first one -- if it doesn't, something is genuinely wrong
            // (not just an off-by-default head count), so this is fatal rather than
            // auto-corrected again.
            long expectedSize = (long) dim2 * dim3 * dim4 * dim5 * (long) sizeof(float);
            if (size != expectedSize) {
                jclass exceptionClass = env->FindClass("java/lang/IllegalStateException");
                if (exceptionClass != nullptr) {
                    char message[256];
                    snprintf(message, sizeof(message),
                             "CacheContainerNative: tensor at index %d has size %ld, expected %ld based on the "
                             "first tensor's shape -- KV-cache tensors are no longer uniform in shape",
                             index, size, expectedSize);
                    env->ThrowNew(exceptionClass, message);
                }
                return;
            }
        }

        //we save buffer pointer in cacheContainer
        cacheContainer[index] = bufferArr;
    }

    void reorder(std::vector<int> * indexes){
        if((*indexes)[0] == 0 && (*indexes)[1] == 1 && (*indexes)[2] == 2 && (*indexes)[3] == 3){
            return;
        }
        float * cacheValueTemp = new float[dim2*dim1*dim3*dim4*dim5];
        for (int i = 0; i < indexes->size(); ++i) {  //the cacheValue vectors are copied and will be reinserted into the cacheValue
            if((*indexes)[i] != i) {
                getCacheBatchValue((*indexes)[i], &cacheValueTemp[getFlatIndexB(i,0,0,0,0)]);
            }
        }
        //reorder is performed
        for (int i = 0; i < indexes->size(); ++i) {
            if((*indexes)[i] != i) {
                setCacheBatchValue(i, &cacheValueTemp[getFlatIndexB(i,0,0,0,0)]);
            }
        }
        delete[] cacheValueTemp;
    }

    jobject getBuffer(JNIEnv *env, int index){
        float * address = cacheContainer[index];
        long capacity = dim2*dim3*dim4*dim5*sizeof(float);
        jobject buffer = env->NewDirectByteBuffer(address, capacity);
        return buffer;
    }

    int getFlatIndex(int i2, int i3, int i4, int i5){
        return i2*dim3*dim4*dim5 + i3*dim4*dim5 + i4*dim5 + i5;
    }

    int getFlatIndexB(int i1, int i2, int i3, int i4, int i5){
        return i1*dim1*dim3*dim4*dim5 + i2*dim3*dim4*dim5 + i3*dim4*dim5 + i4*dim5 + i5;
    }

    ~CacheContainer(){
        delete[] cacheContainer;
    }

private:
    void getCacheBatchValue(int batchIndex, float * cacheBatchValue){
        for(int l=0; l<dim1; l++) {
            for (int k = 0; k < dim3; k++) {
                for (int j = 0; j < dim4; j++) {
                    for (int i = 0; i < dim5; i++) {
                        cacheBatchValue[getFlatIndexB(0,l, k, j, i)] = cacheContainer[l][getFlatIndex(batchIndex, k, j, i)];
                    }
                }
            }
        }
    }

    void setCacheBatchValue(int batchIndex, float * cacheBatchValue){
        for(int l=0; l<dim1; l++) {
            for (int k = 0; k < dim3; k++) {
                for (int j = 0; j < dim4; j++) {
                    for (int i = 0; i < dim5; i++) {
                        cacheContainer[l][getFlatIndex(batchIndex, k, j, i)] = cacheBatchValue[getFlatIndexB(0,l, k, j, i)];
                    }
                }
            }
        }
    }
};



extern "C"
JNIEXPORT jlong JNICALL
Java_nie_translator_rtranslator_tools_nn_CacheContainerNative_initialize(JNIEnv *env,
                                     jobject thiz,
                                     jint dim1, jint dim2,
                                     jint dim3, jint dim4, jint dim5) {
    __android_log_print(ANDROID_LOG_ERROR, "CACHE CONTAINER NATIVE", "%s", "initialization of cache container");
    return (long) new CacheContainer(dim1, dim2, dim3, dim4, dim5);
}

extern "C"
JNIEXPORT void JNICALL
Java_nie_translator_rtranslator_tools_nn_CacheContainerNative_insertValues(JNIEnv *env,
                                     jobject thiz,
                                     jlong cacheContainerPointer, jint index, jobject data) {
    __android_log_print(ANDROID_LOG_ERROR, "CACHE CONTAINER NATIVE", "%s", "inserting data");
    CacheContainer *cacheContainer = (CacheContainer *) cacheContainerPointer;
    cacheContainer->insertData(env, index, data);
}

extern "C"
JNIEXPORT void JNICALL
Java_nie_translator_rtranslator_tools_nn_CacheContainerNative_reorder(JNIEnv *env,
                                       jobject thiz,
                                       jlong cacheContainerPointer, jintArray indexes) {
    __android_log_print(ANDROID_LOG_ERROR, "CACHE CONTAINER NATIVE", "%s", "reordering cache");
    CacheContainer *cacheContainer = (CacheContainer *) cacheContainerPointer;
    std::vector<int> indexesConverted = jintArrayTointVector2(env, indexes);
    cacheContainer->reorder(&indexesConverted);
}

extern "C"
JNIEXPORT jobject JNICALL
Java_nie_translator_rtranslator_tools_nn_CacheContainerNative_getBuffer(JNIEnv *env,
                                      jobject thiz,
                                      jlong cacheContainerPointer, jint index) {
    __android_log_print(ANDROID_LOG_ERROR, "CACHE CONTAINER NATIVE", "%s", "get cache buffer");
    CacheContainer *cacheContainer = (CacheContainer *) cacheContainerPointer;
    return cacheContainer->getBuffer(env, index);
}

extern "C"
JNIEXPORT void JNICALL
Java_nie_translator_rtranslator_tools_nn_CacheContainerNative_close(JNIEnv *env,
                                        jobject thiz,
                                        jlong cacheContainerPointer) {
    __android_log_print(ANDROID_LOG_ERROR, "CACHE CONTAINER NATIVE", "%s", "closing cache container");
    CacheContainer *cacheContainer = (CacheContainer *) cacheContainerPointer;
    delete cacheContainer;
}


std::vector<int> jintArrayTointVector2(JNIEnv* env, jintArray jarray) {
    jsize size = env->GetArrayLength(jarray);
    std::vector<int> vector(size);
    env->GetIntArrayRegion(jarray, jsize{0}, size, &vector[0]);
    std::vector<int> vectorFinal(vector.begin(), vector.end());
    return vectorFinal;
}